//! A small, deterministic stack interpreter for Kiln's parser-independent IR.
//!
//! The interpreter deliberately knows nothing about the frontend.  It consumes only the owned
//! bytecode in `kiln-ir` and reaches the outside world through the versioned `HostOps` trait.  It
//! is intentionally a scalar runtime for the first execution slice; object allocation,
//! exceptions, and async scheduling belong above this VM.

use std::fmt;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use kiln_host_abi::{HostOps, HostValue};
use kiln_ir::{
    BinaryOp, Constant, Instruction, ModuleProgram, Program, UnaryOp, Upvalue, WellKnownSymbol,
};

/// The built-ins the runtime provides natively.
///
/// Kept minimal on purpose. design.md §5 wants built-ins written in JavaScript, and everything
/// here is a placeholder for one -- but nothing can bootstrap a JS built-in until there are
/// prototypes and a loader, so the handful the conformance corpus needs first live here.
/// The well-known symbols, which are the only symbols that exist so far.
///
/// Property keys have separate string, symbol, and VM-private namespaces. In particular, a string
/// beginning with `@@` is still an ordinary program-visible string; it must never alias a symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WellKnown {
    Iterator,
    AsyncIterator,
    HasInstance,
    ToStringTag,
    ToPrimitive,
}

impl WellKnown {
    pub fn property_key(self) -> &'static str {
        match self {
            Self::Iterator => "@@iterator",
            Self::AsyncIterator => "@@asyncIterator",
            Self::HasInstance => "@@hasInstance",
            Self::ToStringTag => "@@toStringTag",
            Self::ToPrimitive => "@@toPrimitive",
        }
    }

    /// What `String(symbol)` shows, which spells the well-known ones out.
    fn description(self) -> &'static str {
        match self {
            Self::Iterator => "Symbol.iterator",
            Self::AsyncIterator => "Symbol.asyncIterator",
            Self::HasInstance => "Symbol.hasInstance",
            Self::ToStringTag => "Symbol.toStringTag",
            Self::ToPrimitive => "Symbol.toPrimitive",
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
    serial: u64,
    description: Option<String>,
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
            Self::Unique(symbol) => format!("@@symbol:{}", symbol.serial),
        }
    }

    fn description(&self) -> &str {
        match self {
            Self::WellKnown(symbol) => symbol.description(),
            Self::Unique(symbol) => symbol.description.as_deref().unwrap_or(""),
        }
    }
}

impl PartialEq for Symbol {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::WellKnown(left), Self::WellKnown(right)) => left == right,
            (Self::Unique(left), Self::Unique(right)) => Rc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl Eq for Symbol {}

/// Keys used by the object store. `Internal` is inaccessible through JavaScript syntax and is
/// used for iterator state and other VM bookkeeping. Symbols remain values here (rather than
/// being converted to printable strings), preserving identity for unique symbols.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropertyKey {
    String(String),
    Symbol(Symbol),
    Internal(InternalKey),
}

impl PartialEq<&str> for PropertyKey {
    fn eq(&self, other: &&str) -> bool {
        matches!(self, Self::String(value) if value == *other)
    }
}
impl PartialEq<PropertyKey> for &str {
    fn eq(&self, other: &PropertyKey) -> bool {
        other == self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InternalKey {
    Slot(String),
    /// An engine-private alias can retain a symbol's identity without exposing it to JavaScript.
    Symbol(Symbol),
}

impl From<&str> for PropertyKey {
    fn from(key: &str) -> Self {
        Self::String(key.to_owned())
    }
}
impl From<&&str> for PropertyKey {
    fn from(key: &&str) -> Self {
        Self::String((*key).to_owned())
    }
}
impl From<String> for PropertyKey {
    fn from(key: String) -> Self {
        Self::String(key)
    }
}
impl From<&String> for PropertyKey {
    fn from(key: &String) -> Self {
        Self::String(key.clone())
    }
}
impl From<Symbol> for PropertyKey {
    fn from(key: Symbol) -> Self {
        Self::Symbol(key)
    }
}
impl From<InternalKey> for PropertyKey {
    fn from(key: InternalKey) -> Self {
        Self::Internal(key)
    }
}
impl From<&PropertyKey> for PropertyKey {
    fn from(key: &PropertyKey) -> Self {
        key.clone()
    }
}

/// VM-private bookkeeping never enters the program-visible string namespace.
fn internal_key(name: &str) -> PropertyKey {
    PropertyKey::Internal(InternalKey::Slot(name.to_owned()))
}

/// Render a private key's source identifier without exposing its engine-private scope.
///
/// Graph execution prefixes the compiler's class scope with a module namespace, but private
/// identifiers themselves cannot contain `:`, so the final segment is always the user-written
/// name. Keeping this separate from [`internal_key`] ensures diagnostics never become part of the
/// storage-key format.
fn private_display_name(name: &str) -> &str {
    name.rsplit_once(':')
        .map_or(name, |(_, identifier)| identifier)
}

fn iterator_key() -> PropertyKey {
    PropertyKey::Symbol(Symbol::WellKnown(WellKnown::Iterator))
}

fn key_value(key: &PropertyKey) -> Value {
    match key {
        PropertyKey::String(text) => Value::String(text.clone()),
        PropertyKey::Symbol(symbol) => Value::Symbol(symbol.clone()),
        PropertyKey::Internal(_) => Value::Undefined,
    }
}

fn string_key(key: &PropertyKey) -> Option<&str> {
    match key {
        PropertyKey::String(text) => Some(text),
        _ => None,
    }
}

/// The error constructors a realm provides.
///
/// One enum rather than one `Native` each, because everything about them is the same shape: a
/// name, a prototype that chains to `Error.prototype`, and a constructor reachable from an
/// instance through that prototype. The VM also raises them itself, and having the kinds in one
/// type is what lets it say *which* without a second table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Error,
    Type,
    Range,
    Reference,
    Syntax,
}

impl ErrorKind {
    /// Every kind, in declaration order, so the realm can build all their prototypes.
    const ALL: [Self; 5] = [
        Self::Error,
        Self::Type,
        Self::Range,
        Self::Reference,
        Self::Syntax,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Type => "TypeError",
            Self::Range => "RangeError",
            Self::Reference => "ReferenceError",
            Self::Syntax => "SyntaxError",
        }
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|kind| *kind == self)
            .expect("every kind is in ALL")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Native {
    Error(ErrorKind),
    SymbolConstructor,
    ArrayConstructor,
    ObjectConstructor,
    Json,
    ArrayIsArray,
    ArrayFrom,
    ObjectKeys,
    ObjectSetPrototypeOf,
    ObjectGetPrototypeOf,
    StringConstructor,
    JsonParse,
    JsonStringify,
    /// The `Math` namespace, and the functions reached through it.
    ///
    /// `Math.random` is here and is *deterministic*: a fixed seed and a fixed generator, the same
    /// in both engines, for the same reason the clock is virtual. See ADR 0026.
    Math,
    MathRandom,
    MathSinh,
    MathCosh,
    MathTanh,
    MathAsinh,
    MathAcosh,
    MathAtanh,
    MathLog1p,
    MathExpm1,
    MathFround,
    MathClz32,
    MathImul,
    MathAbs,
    MathFloor,
    DateToJson,
    MathHypot,
    MathAtan2,
    MathSin,
    MathCos,
    MathTan,
    MathAsin,
    MathAcos,
    MathAtan,
    MathCeil,
    MathRound,
    MathTrunc,
    MathSign,
    MathSqrt,
    MathCbrt,
    MathPow,
    MathMin,
    MathMax,
    MathLog,
    MathLog2,
    MathLog10,
    MathExp,
    NumberConstructor,
    NumberIsInteger,
    NumberIsSafeInteger,
    /// `Number.isNaN`, which does not coerce -- unlike the global `isNaN`, which does.
    NumberIsNaN,
    NumberIsFinite,
    /// `Number.prototype.toString`, `toFixed` and `valueOf`.
    NumberToString,
    NumberToFixed,
    NumberValueOf,
    StringAt,
    StringCharAt,
    StringCharCodeAt,
    StringPadStart,
    StringPadEnd,
    StringReplace,
    StringReplaceAll,
    StringLastIndexOf,
    StringTrimStart,
    StringTrimEnd,
    StringConcat,
    ArrayAt,
    ArrayConcat,
    ArrayReverse,
    ArrayShift,
    ArrayUnshift,
    ArrayLastIndexOf,
    ArrayFill,
    ArraySplice,
    ArrayCopyWithin,
    /// `toString`, which is `join` with the separator fixed rather than defaulted -- a program can
    /// pass one to `join` and cannot to `toString`.
    ArrayJoinDefault,
    SetTimeout,
    SetInterval,
    /// `clearTimeout` and `clearInterval` are one operation: both remove a scheduled callback by
    /// the id it was given, and the language does not distinguish which kind it was.
    /// `clearTimeout` and `clearInterval` do the same thing and are two variants because they
    /// are two functions: a program can print either name, and `f === g` must be false for them.
    ClearTimeout,
    ClearInterval,
    QueueMicrotask,
    DateConstructor,
    DateNow,
    DateUtc,
    DateParse,
    /// Every reader of a date part. One native with the field it reads, because they differ only
    /// in which number they take out of the same decomposition.
    DateField(DateField),
    DateToIso,
    DateToString,
    WeakMapConstructor,
    WeakSetConstructor,
    /// `performance`, whose `now` reads the same virtual clock `Date.now` does.
    Performance,
    PerformanceNow,
    SymbolFor,
    SymbolKeyFor,
    ObjectAssign,
    ObjectEntries,
    ObjectValues,
    ObjectFreeze,
    ObjectIsFrozen,
    ObjectCreate,
    ObjectDefineProperty,
    ObjectFromEntries,
    ArrayOf,
    StringFromCharCode,
    Reflect,
    ReflectOwnKeys,
    ReflectGet,
    ReflectSet,
    ReflectApply,
    ObjectHasOwn,
    BigIntConstructor,
    BigIntToString,
    ObjectIs,
    /// A `setX` on a date. Writes through the shared instant, so the object mutates in place.
    DateSet(DateField),
    ObjectHasOwnProperty,
    ObjectIsPrototypeOf,
    ObjectPropertyIsEnumerable,
    ObjectToStringTag,
    ObjectValueOf,
    /// `ToString` of the receiver, which is what a primitive's `toString` returns.
    PrimitiveToString,
    /// `Object.prototype`, `Array.prototype` and the rest, as method tables.
    ///
    /// Not real prototype objects: the realm has no chain to hang built-ins on yet (design.md
    /// §5). A marker is enough for what programs actually do with them, which is reach for one
    /// method and `call` it onto a receiver of their own.
    PrototypeOf(PrototypeTable),
    StringRaw,
    RegExpToString,
    Escape,
    Unescape,
    EncodeUri,
    EncodeUriComponent,
    DecodeUri,
    DecodeUriComponent,
    StringSubstring,
    StringSubstr,
    CollectionKeys,
    CollectionForEach,
    CollectionEntries,
    /// `Function`, which programs reach for through `Function.prototype`.
    FunctionConstructor,
    /// `Proxy`, which is only ever reached through `new`.
    ProxyConstructor,
    /// `ArrayBuffer`, and one constructor per view kind.
    ArrayBufferConstructor,
    TypedArrayConstructor(TypedKind),
    TypedArrayOf(TypedKind),
    TypedArrayFrom(TypedKind),
    TypedArraySet,
    TypedArraySubarray,
    TypedArrayFill,
    ProxyRevocable,
    /// The `revoke` half of a revocable pair. Its receiver is the proxy it turns off.
    ProxyRevoke,
    /// `Function.prototype.toString`.
    FunctionToString,
    FunctionCall,
    FunctionApply,
    FunctionBind,
    ObjectPreventExtensions,
    ObjectIsExtensible,
    ObjectSeal,
    ObjectIsSealed,
    ObjectGetOwnPropertySymbols,
    ObjectDefineProperties,
    ObjectGetOwnPropertyDescriptor,
    ObjectGetOwnPropertyDescriptors,
    ArrayFlat,
    ArrayEntries,
    ArrayKeys,
    StringCodePointAt,
    StringLocaleCompare,
    StringNormalize,
    RegExpConstructor,
    RegExpTest,
    RegExpExec,
    StringMatch,
    StringMatchAll,
    StringSearch,
    PromiseConstructor,
    PromiseResolve,
    PromiseReject,
    PromiseAll,
    PromiseAllSettled,
    PromiseRace,
    PromiseAny,
    PromiseThen,
    PromiseCatch,
    PromiseFinally,
    BooleanConstructor,
    ParseInt,
    ParseFloat,
    IsNaN,
    IsFinite,
    /// `Array.prototype.join`.
    ArrayJoin,
    /// `next` on an array iterator.
    ArrayIteratorNext,
    /// `String.prototype[Symbol.iterator]`, which walks code points rather than UTF-16 units.
    StringValues,
    StringIteratorNext,
    StringToUpperCase,
    StringToLowerCase,
    StringIncludes,
    StringIndexOf,
    StringSlice,
    StringSplit,
    StringTrim,
    StringStartsWith,
    StringEndsWith,
    StringRepeat,
    ArrayPush,
    ArrayPop,
    ArrayIndexOf,
    ArrayIncludes,
    ArraySlice,
    SetConstructor,
    MapConstructor,
    SetAdd,
    SetHas,
    SetDelete,
    CollectionClear,
    ObjectOwnNames,
    MapSet,
    MapGet,
    MapHas,
    MapDelete,
    CollectionValues,
    CollectionIteratorNext,
    /// An absent element of an array: the hole in `[1, , 3]`.
    ///
    /// Not `undefined`, because the two are distinguishable: `1 in [1, , 3]` is false while
    /// `1 in [1, undefined, 3]` is true, and every method that walks elements skips a hole and
    /// visits an undefined. Reading one still *yields* undefined, which is why the marker never
    /// escapes into an expression.
    Hole,
    /// What a `let` or `const` slot holds before its declaration runs.
    ///
    /// A marker rather than `undefined`, because the two are different answers: reading a binding
    /// in its dead zone is an error, and reading one initialised to `undefined` is not. The
    /// compiler catches a *direct* read at compile time; this catches the read a nested function
    /// makes when it is called too early, which no compile-time check can see.
    Uninitialized,
    /// The value thrown into a generator by `.return()`.
    ///
    /// A marker rather than a flag because the unwinding it needs is exactly the unwinding an
    /// exception needs -- through every `finally` on the way out -- and reusing the exception
    /// path means there is one implementation of that rather than two. No expression produces
    /// it, so a program cannot catch it by accident.
    ReturnSignal,
    /// A deleted slot in a Set or Map.
    ///
    /// Deletion leaves a hole rather than compacting, because compacting a dense array under a
    /// live iterator shifts the elements past the cursor and skips one. That is the exact bug
    /// g13 exists to catch. Never reachable from JavaScript: no expression produces it.
    Tombstone,
    /// Returns its own receiver, so an iterator is iterable.
    IteratorSelf,
    /// `Array.prototype.values`, which is also `Array.prototype[Symbol.iterator]` -- the same
    /// function object, which is what `[][Symbol.iterator] === [].values` asserts.
    ArrayValues,
    /// A V8 extension the corpus checks for. Present, and a no-op: kiln has no frames to capture,
    /// so the honest implementation is one that exists and does nothing.
    ErrorCaptureStackTrace,
    /// `Error.prototype.toString`, which renders `name: message`.
    ErrorToString,
}

impl Native {
    /// Whether this built-in *changes* the array it is given, rather than only reading it.
    ///
    /// Redirected onto an array-like, such a method has nowhere to write; onto a proxy it does,
    /// through the `set` trap, and the write is the whole point of calling it.
    fn mutates_an_array(self) -> bool {
        matches!(
            self,
            Self::ArrayPush
                | Self::ArrayPop
                | Self::ArrayShift
                | Self::ArrayUnshift
                | Self::ArrayReverse
                | Self::ArrayFill
        )
    }

    /// Whether this built-in changes how many elements its array has.
    ///
    /// A typed array's length is fixed, so these are the ones it does not have. The other
    /// mutators -- `reverse`, `fill` -- keep the length and are fine on one.
    fn changes_array_length(self) -> bool {
        matches!(
            self,
            Self::ArrayPush | Self::ArrayPop | Self::ArrayShift | Self::ArrayUnshift
        )
    }

    /// Whether this built-in hands back a *new* array rather than a value out of the old one.
    ///
    /// Called on a typed array, such a method returns a typed array of the same kind. That is
    /// the difference a console shows, and the reason this classification exists.
    fn returns_a_new_array(self) -> bool {
        matches!(self, Self::ArraySlice | Self::ArrayConcat | Self::ArrayFlat)
    }

    /// Whether this built-in rearranges its array in place, keeping the length.
    fn rearranges_in_place(self) -> bool {
        matches!(self, Self::ArrayReverse)
    }

    /// Whether this built-in reads its receiver as an array, and so should accept an array-like
    /// one when it is redirected through `call` or `apply`.
    ///
    /// The constructor and the statics are excluded: `Array.from` and `Array.isArray` take their
    /// subject as an argument, and coercing a receiver they never read would be noise.
    fn walks_an_array(self) -> bool {
        matches!(
            self,
            Self::ArrayAt
                | Self::ArrayConcat
                | Self::ArrayEntries
                | Self::ArrayFill
                | Self::ArrayFlat
                | Self::ArrayIncludes
                | Self::ArrayIndexOf
                | Self::ArrayJoin
                | Self::ArrayKeys
                | Self::ArrayLastIndexOf
                | Self::ArrayPop
                | Self::ArrayPush
                | Self::ArrayReverse
                | Self::ArrayShift
                | Self::ArraySlice
                | Self::ArrayUnshift
                | Self::ArrayValues
        )
    }

    fn property(self, name: &str) -> Option<Value> {
        match (self, name) {
            (Self::Error(_), "captureStackTrace") => {
                Some(Value::Native(Self::ErrorCaptureStackTrace))
            }
            // `e.constructor.name` reaches this: the instance finds its constructor through the
            // prototype, and the constructor knows which kind it is.
            (Self::Error(kind), "name") => Some(Value::String(kind.name().to_owned())),
            (Self::ArrayConstructor, "isArray") => Some(Value::Native(Self::ArrayIsArray)),
            (Self::ArrayConstructor, "from") => Some(Value::Native(Self::ArrayFrom)),
            (Self::ObjectConstructor, "keys") => Some(Value::Native(Self::ObjectKeys)),
            // `getOwnPropertyNames` differs from `keys` only for non-enumerable properties, and
            // nothing here can make one yet -- so they are the same function until that changes.
            // Not `Object.keys`: this reports every own property and that one reports only
            // the enumerated ones. They were the same function until an error object gave them
            // different answers.
            (Self::ObjectConstructor, "getOwnPropertyNames") => {
                Some(Value::Native(Self::ObjectOwnNames))
            }
            (Self::ObjectConstructor, "assign") => Some(Value::Native(Self::ObjectAssign)),
            (Self::ObjectConstructor, "entries") => Some(Value::Native(Self::ObjectEntries)),
            (Self::ObjectConstructor, "values") => Some(Value::Native(Self::ObjectValues)),
            (Self::ObjectConstructor, "freeze") => Some(Value::Native(Self::ObjectFreeze)),
            (Self::ObjectConstructor, "isFrozen") => Some(Value::Native(Self::ObjectIsFrozen)),
            (Self::ObjectConstructor, "create") => Some(Value::Native(Self::ObjectCreate)),
            (Self::ObjectConstructor, "defineProperty") => {
                Some(Value::Native(Self::ObjectDefineProperty))
            }
            (Self::ObjectConstructor, "fromEntries") => {
                Some(Value::Native(Self::ObjectFromEntries))
            }
            (Self::ArrayConstructor, "of") => Some(Value::Native(Self::ArrayOf)),
            (Self::StringConstructor, "raw") => Some(Value::Native(Self::StringRaw)),
            (Self::StringConstructor, "fromCharCode") => {
                Some(Value::Native(Self::StringFromCharCode))
            }
            // `Reflect` is mostly the `Object` statics with a different failure convention, and
            // the ones a program actually reaches are the ones that behave identically here.
            (Self::Reflect, "ownKeys") => Some(Value::Native(Self::ReflectOwnKeys)),
            (Self::Reflect, "get") => Some(Value::Native(Self::ReflectGet)),
            (Self::Reflect, "set") => Some(Value::Native(Self::ReflectSet)),
            (Self::Reflect, "has") => Some(Value::Native(Self::ObjectHasOwn)),
            (Self::Reflect, "apply") => Some(Value::Native(Self::ReflectApply)),
            (Self::Reflect, "getPrototypeOf") => Some(Value::Native(Self::ObjectGetPrototypeOf)),
            (Self::Reflect, "setPrototypeOf") => Some(Value::Native(Self::ObjectSetPrototypeOf)),
            (Self::Reflect, "defineProperty") => Some(Value::Native(Self::ObjectDefineProperty)),
            (Self::ObjectConstructor, "hasOwn") => Some(Value::Native(Self::ObjectHasOwn)),
            (Self::ObjectConstructor, "is") => Some(Value::Native(Self::ObjectIs)),
            (Self::ObjectConstructor, "prototype") => {
                Some(Value::Native(Self::PrototypeOf(PrototypeTable::Object)))
            }
            (Self::ArrayConstructor, "prototype") => {
                Some(Value::Native(Self::PrototypeOf(PrototypeTable::Array)))
            }
            (Self::StringConstructor, "prototype") => {
                Some(Value::Native(Self::PrototypeOf(PrototypeTable::String)))
            }
            (Self::NumberConstructor, "prototype") => {
                Some(Value::Native(Self::PrototypeOf(PrototypeTable::Number)))
            }
            (Self::RegExpConstructor, "prototype") => {
                Some(Value::Native(Self::PrototypeOf(PrototypeTable::RegExp)))
            }
            (Self::DateConstructor, "prototype") => {
                Some(Value::Native(Self::PrototypeOf(PrototypeTable::Date)))
            }
            // A method read off one of these tables is unbound, exactly as reading it off an
            // instance is: `call` and `apply` are what supply the receiver.
            // `Proxy.revocable(target, handler)` hands back the proxy and the switch that turns
            // it off, as a pair.
            (Self::ProxyConstructor, "revocable") => Some(Value::Native(Self::ProxyRevocable)),
            (Self::TypedArrayConstructor(kind), "of") => {
                Some(Value::Native(Self::TypedArrayOf(kind)))
            }
            (Self::TypedArrayConstructor(kind), "from") => {
                Some(Value::Native(Self::TypedArrayFrom(kind)))
            }
            (Self::TypedArrayConstructor(kind), "BYTES_PER_ELEMENT") => {
                Some(Value::Number(kind.width() as f64))
            }
            // Every function inherits from `Function.prototype`, which is what the interop
            // shims test when they clone a module's shape.
            (Self::FunctionConstructor, "prototype") => {
                Some(Value::Native(Self::PrototypeOf(PrototypeTable::Function)))
            }
            // A method table's `constructor` is the built-in it belongs to, which is what
            // `Object.prototype.constructor` has to answer for a plain-object check to work.
            (Self::PrototypeOf(table), "constructor") => Some(Value::Native(match table {
                PrototypeTable::Object => Self::ObjectConstructor,
                PrototypeTable::Array => Self::ArrayConstructor,
                PrototypeTable::String => Self::StringConstructor,
                PrototypeTable::Number => Self::NumberConstructor,
                PrototypeTable::Function => Self::FunctionConstructor,
                PrototypeTable::RegExp => Self::RegExpConstructor,
                PrototypeTable::Date => Self::DateConstructor,
            })),
            (Self::PrototypeOf(table), _) => Some(match table {
                PrototypeTable::Object => object_property(name),
                PrototypeTable::Array => array_property(name),
                PrototypeTable::String => string_property(name),
                PrototypeTable::Number => number_property(name),
                PrototypeTable::Function => function_property(name),
                PrototypeTable::RegExp => regexp_method(name),
                PrototypeTable::Date => date_method(name),
            }),
            (Self::ObjectConstructor, "preventExtensions") => {
                Some(Value::Native(Self::ObjectPreventExtensions))
            }
            (Self::ObjectConstructor, "isExtensible") => {
                Some(Value::Native(Self::ObjectIsExtensible))
            }
            // Sealing is preventing extensions plus making each property non-configurable, and
            // configurability is not tracked per-property, so the two coincide here. The
            // difference only shows through `delete`, which `frozen` already covers.
            (Self::ObjectConstructor, "seal") => Some(Value::Native(Self::ObjectSeal)),
            (Self::ObjectConstructor, "isSealed") => Some(Value::Native(Self::ObjectIsSealed)),
            (Self::ObjectConstructor, "getOwnPropertySymbols") => {
                Some(Value::Native(Self::ObjectGetOwnPropertySymbols))
            }
            (Self::ObjectConstructor, "defineProperties") => {
                Some(Value::Native(Self::ObjectDefineProperties))
            }
            (Self::ObjectConstructor, "getOwnPropertyDescriptor") => {
                Some(Value::Native(Self::ObjectGetOwnPropertyDescriptor))
            }
            (Self::ObjectConstructor, "getOwnPropertyDescriptors") => {
                Some(Value::Native(Self::ObjectGetOwnPropertyDescriptors))
            }
            (Self::ObjectConstructor, "setPrototypeOf") => {
                Some(Value::Native(Self::ObjectSetPrototypeOf))
            }
            (Self::ObjectConstructor, "getPrototypeOf") => {
                Some(Value::Native(Self::ObjectGetPrototypeOf))
            }
            (Self::Math, "abs") => Some(Value::Native(Self::MathAbs)),
            (Self::Math, "floor") => Some(Value::Native(Self::MathFloor)),
            (Self::Math, "ceil") => Some(Value::Native(Self::MathCeil)),
            (Self::Math, "round") => Some(Value::Native(Self::MathRound)),
            (Self::Math, "trunc") => Some(Value::Native(Self::MathTrunc)),
            (Self::Math, "sign") => Some(Value::Native(Self::MathSign)),
            (Self::Math, "sqrt") => Some(Value::Native(Self::MathSqrt)),
            (Self::Math, "cbrt") => Some(Value::Native(Self::MathCbrt)),
            (Self::Math, "random") => Some(Value::Native(Self::MathRandom)),
            (Self::Math, "sinh") => Some(Value::Native(Self::MathSinh)),
            (Self::Math, "cosh") => Some(Value::Native(Self::MathCosh)),
            (Self::Math, "tanh") => Some(Value::Native(Self::MathTanh)),
            (Self::Math, "asinh") => Some(Value::Native(Self::MathAsinh)),
            (Self::Math, "acosh") => Some(Value::Native(Self::MathAcosh)),
            (Self::Math, "atanh") => Some(Value::Native(Self::MathAtanh)),
            (Self::Math, "log1p") => Some(Value::Native(Self::MathLog1p)),
            (Self::Math, "expm1") => Some(Value::Native(Self::MathExpm1)),
            (Self::Math, "fround") => Some(Value::Native(Self::MathFround)),
            (Self::Math, "clz32") => Some(Value::Native(Self::MathClz32)),
            (Self::Math, "imul") => Some(Value::Native(Self::MathImul)),
            (Self::Math, "pow") => Some(Value::Native(Self::MathPow)),
            (Self::Math, "min") => Some(Value::Native(Self::MathMin)),
            (Self::Math, "max") => Some(Value::Native(Self::MathMax)),
            (Self::Math, "log") => Some(Value::Native(Self::MathLog)),
            (Self::Math, "log2") => Some(Value::Native(Self::MathLog2)),
            (Self::Math, "log10") => Some(Value::Native(Self::MathLog10)),
            (Self::Math, "exp") => Some(Value::Native(Self::MathExp)),
            (Self::Math, "PI") => Some(Value::Number(std::f64::consts::PI)),
            (Self::Math, "E") => Some(Value::Number(std::f64::consts::E)),
            (Self::Math, "LN2") => Some(Value::Number(std::f64::consts::LN_2)),
            (Self::Math, "LN10") => Some(Value::Number(std::f64::consts::LN_10)),
            (Self::Math, "SQRT2") => Some(Value::Number(std::f64::consts::SQRT_2)),
            (Self::Math, "SQRT1_2") => Some(Value::Number(std::f64::consts::FRAC_1_SQRT_2)),
            (Self::Math, "LOG2E") => Some(Value::Number(std::f64::consts::LOG2_E)),
            (Self::Math, "LOG10E") => Some(Value::Number(std::f64::consts::LOG10_E)),
            (Self::Math, "hypot") => Some(Value::Native(Self::MathHypot)),
            (Self::Math, "atan2") => Some(Value::Native(Self::MathAtan2)),
            (Self::Math, "sin") => Some(Value::Native(Self::MathSin)),
            (Self::Math, "cos") => Some(Value::Native(Self::MathCos)),
            (Self::Math, "tan") => Some(Value::Native(Self::MathTan)),
            (Self::Math, "asin") => Some(Value::Native(Self::MathAsin)),
            (Self::Math, "acos") => Some(Value::Native(Self::MathAcos)),
            (Self::Math, "atan") => Some(Value::Native(Self::MathAtan)),
            (Self::NumberConstructor, "isInteger") => Some(Value::Native(Self::NumberIsInteger)),
            (Self::NumberConstructor, "isSafeInteger") => {
                Some(Value::Native(Self::NumberIsSafeInteger))
            }
            // `Number.isNaN` is not the global `isNaN`: it does not coerce, so
            // `Number.isNaN("x")` is false where `isNaN("x")` is true. Same code, different
            // entry points would be wrong -- these are genuinely two functions.
            (Self::NumberConstructor, "isNaN") => Some(Value::Native(Self::NumberIsNaN)),
            (Self::NumberConstructor, "isFinite") => Some(Value::Native(Self::NumberIsFinite)),
            (Self::NumberConstructor, "parseInt") => Some(Value::Native(Self::ParseInt)),
            (Self::NumberConstructor, "parseFloat") => Some(Value::Native(Self::ParseFloat)),
            (Self::NumberConstructor, "MAX_SAFE_INTEGER") => {
                Some(Value::Number(9_007_199_254_740_991.0))
            }
            (Self::NumberConstructor, "MIN_SAFE_INTEGER") => {
                Some(Value::Number(-9_007_199_254_740_991.0))
            }
            (Self::NumberConstructor, "MAX_VALUE") => Some(Value::Number(f64::MAX)),
            (Self::NumberConstructor, "MIN_VALUE") => Some(Value::Number(5e-324)),
            (Self::NumberConstructor, "EPSILON") => Some(Value::Number(f64::EPSILON)),
            (Self::NumberConstructor, "POSITIVE_INFINITY") => Some(Value::Number(f64::INFINITY)),
            (Self::NumberConstructor, "NEGATIVE_INFINITY") => {
                Some(Value::Number(f64::NEG_INFINITY))
            }
            (Self::NumberConstructor, "NaN") => Some(Value::Number(f64::NAN)),
            (Self::Performance, "now") => Some(Value::Native(Self::PerformanceNow)),
            (Self::DateConstructor, "now") => Some(Value::Native(Self::DateNow)),
            (Self::DateConstructor, "UTC") => Some(Value::Native(Self::DateUtc)),
            (Self::DateConstructor, "parse") => Some(Value::Native(Self::DateParse)),
            (Self::PromiseConstructor, "resolve") => Some(Value::Native(Self::PromiseResolve)),
            (Self::PromiseConstructor, "reject") => Some(Value::Native(Self::PromiseReject)),
            (Self::PromiseConstructor, "all") => Some(Value::Native(Self::PromiseAll)),
            (Self::PromiseConstructor, "allSettled") => {
                Some(Value::Native(Self::PromiseAllSettled))
            }
            (Self::PromiseConstructor, "race") => Some(Value::Native(Self::PromiseRace)),
            (Self::PromiseConstructor, "any") => Some(Value::Native(Self::PromiseAny)),
            (Self::Json, "parse") => Some(Value::Native(Self::JsonParse)),
            (Self::Json, "stringify") => Some(Value::Native(Self::JsonStringify)),
            (Self::SymbolConstructor, "for") => Some(Value::Native(Self::SymbolFor)),
            (Self::SymbolConstructor, "keyFor") => Some(Value::Native(Self::SymbolKeyFor)),
            // The well-known symbols a program can name. Each is the *same* symbol every time,
            // which is the whole point of them.
            (Self::SymbolConstructor, "asyncIterator") => {
                Some(Value::Symbol(Symbol::WellKnown(WellKnown::AsyncIterator)))
            }
            (Self::SymbolConstructor, "hasInstance") => {
                Some(Value::Symbol(Symbol::WellKnown(WellKnown::HasInstance)))
            }
            (Self::SymbolConstructor, "toStringTag") => {
                Some(Value::Symbol(Symbol::WellKnown(WellKnown::ToStringTag)))
            }
            (Self::SymbolConstructor, "toPrimitive") => {
                Some(Value::Symbol(Symbol::WellKnown(WellKnown::ToPrimitive)))
            }
            (Self::SymbolConstructor, "iterator") => {
                Some(Value::Symbol(Symbol::WellKnown(WellKnown::Iterator)))
            }
            _ => None,
        }
    }
}

/// An ordinary object: string-keyed properties in insertion order.
///
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

#[derive(Clone, Debug)]
pub struct ObjectData {
    properties: Vec<(PropertyKey, Property)>,
    /// Own properties that exist but are not *enumerated*: readable by name, invisible to
    /// `Object.keys`, `for...in`, spread and JSON.
    ///
    /// A list rather than a flag on each slot because it is empty for almost every object --
    /// only an error carries one today. `entries` is the enumeration accessor and filters here,
    /// so every caller that walks an object inherits the rule; `keys` does not, because
    /// `getOwnPropertyNames` is supposed to see them.
    non_enumerable: Vec<PropertyKey>,
    /// `Object.preventExtensions` and `Object.seal`. Weaker than frozen: the properties that
    /// are already there stay writable, only the shape is closed.
    extensible: bool,
    /// `Object.freeze`. A frozen object silently ignores every write, which is what sloppy mode
    /// does; strict mode throws, and reaching that needs the write path to know which it is in.
    frozen: bool,
    /// The `[[Prototype]]` link. Reads fall through it; writes never do -- assigning a property
    /// that exists only on the prototype creates an own property rather than mutating the shared
    /// one, which is what keeps two instances of a class independent.
    prototype: Option<Value>,
}

impl Default for ObjectData {
    fn default() -> Self {
        Self {
            properties: Vec::new(),
            non_enumerable: Vec::new(),
            // Objects start open; closing one is always something a program asked for.
            extensible: true,
            frozen: false,
            prototype: None,
        }
    }
}

/// How far a prototype chain is followed before a read gives up.
///
/// A chain this long is a program that built a cycle -- `setPrototypeOf` refuses to make one, but
/// nothing stops an object graph from being deep -- and the alternative to a bound is exhausting
/// the Rust stack, which aborts the process instead of reporting anything.
const MAX_PROTOTYPE_DEPTH: usize = 10_000;

impl ObjectData {
    /// An own property slot, ignoring the prototype chain.
    pub fn get_own_property<K: Into<PropertyKey>>(&self, key: K) -> Option<&Property> {
        let key = key.into();
        self.properties
            .iter()
            .find(|(name, _)| name == &key)
            .map(|(_, property)| property)
    }

    /// An own *data* property, ignoring the prototype chain and any accessor.
    ///
    /// For the VM's own slots, which are always data and never reached by program syntax. A
    /// program-visible read has to go through the interpreter, because an accessor is a call.
    pub fn get_own<K: Into<PropertyKey>>(&self, key: K) -> Option<&Value> {
        match self.get_own_property(key) {
            Some(Property::Value(value)) => Some(value),
            _ => None,
        }
    }

    /// A property slot, following the prototype chain.
    ///
    /// Iterative and bounded rather than recursive. A chain is built by the program, so its
    /// length is the program's business -- but the recursion was on the *Rust* stack, where
    /// running out is an abort rather than an error, and a chain long enough to do that is
    /// reachable from ordinary code.
    pub fn lookup<K: Into<PropertyKey>>(&self, key: K) -> Option<Property> {
        let key = key.into();
        if let Some(own) = self.get_own_property(key.clone()) {
            return Some(own.clone());
        }
        let mut current = self.prototype.clone();
        for _ in 0..MAX_PROTOTYPE_DEPTH {
            let found = match &current {
                Some(Value::Object(parent)) => {
                    let parent = parent.borrow();
                    parent
                        .get_own_property(key.clone())
                        .cloned()
                        .ok_or_else(|| parent.prototype.clone())
                }
                Some(Value::Function(parent)) => {
                    let parent = parent.properties.borrow();
                    parent
                        .get_own_property(key.clone())
                        .cloned()
                        .ok_or_else(|| parent.prototype.clone())
                }
                _ => return None,
            };
            match found {
                Ok(property) => return Some(property),
                Err(next) => current = next,
            }
        }
        None
    }

    /// Define an own data property, replacing whatever was there.
    ///
    /// Unlike [`ObjectData::set`] this never consults an inherited accessor: `defineProperty`
    /// *defines*, where an assignment *assigns*, and the difference is exactly the idiom where a
    /// prototype getter replaces itself with an own value on first read. Routing that through
    /// the prototype's setter calls it again, and again.
    pub fn define_value<K: Into<PropertyKey>>(&mut self, key: K, value: Value) {
        let key = key.into();
        if self.frozen {
            return;
        }
        match self.properties.iter_mut().find(|(name, _)| name == &key) {
            Some((_, slot)) => *slot = Property::Value(value),
            None => self.set(key, value),
        }
    }

    /// Define an accessor, merging with a half already defined under the same key.
    ///
    /// Merging matters because `get x()` and `set x()` are written separately and describe one
    /// property; defining the second must not discard the first.
    pub fn define_accessor<K: Into<PropertyKey>>(
        &mut self,
        key: K,
        getter: Option<Value>,
        setter: Option<Value>,
    ) {
        let key = key.into();
        let existing = match self.get_own_property(key.clone()) {
            Some(Property::Accessor { get, set }) => (get.clone(), set.clone()),
            _ => (None, None),
        };
        let property = Property::Accessor {
            get: getter.or(existing.0),
            set: setter.or(existing.1),
        };
        match self.properties.iter_mut().find(|(name, _)| name == &key) {
            Some((_, slot)) => *slot = property,
            None => self.insert_ordered(key, property),
        }
    }

    /// A property, following the prototype chain.
    ///
    /// Returns an owned value rather than a reference because the answer may live in another
    /// object behind an `Rc<RefCell<_>>`, whose borrow cannot outlive this call.
    pub fn get<K: Into<PropertyKey>>(&self, key: K) -> Option<Value> {
        // Same walk as `lookup`, and data-only: this is what the VM's own reads use.
        match self.lookup(key) {
            Some(Property::Value(value)) => Some(value),
            _ => None,
        }
    }

    pub fn prototype(&self) -> Option<&Value> {
        self.prototype.as_ref()
    }

    /// Link this object to a prototype, refusing to close a cycle.
    ///
    /// The specification makes `Object.setPrototypeOf` a TypeError when it would create one.
    /// Here the reason is more immediate: a cycle makes every miss walk forever.
    pub fn set_prototype(&mut self, prototype: Value) {
        let mut current = Some(prototype.clone());
        for _ in 0..MAX_PROTOTYPE_DEPTH {
            let next = match &current {
                Some(Value::Object(parent)) => {
                    if std::ptr::eq(parent.as_ptr(), std::ptr::from_ref(self)) {
                        return;
                    }
                    parent.borrow().prototype.clone()
                }
                Some(Value::Function(parent)) => parent.properties.borrow().prototype.clone(),
                _ => break,
            };
            current = next;
        }
        self.prototype = Some(prototype);
    }

    /// Insert or overwrite, keeping the store in property-key enumeration order.
    ///
    /// Re-assigning a property does not move it to the end in JavaScript, so it must not here.
    /// A new key is placed in the appropriate stable partition: integer indices first (ascending),
    /// then other strings in insertion order, then symbols in insertion order. Internal keys are
    /// kept with symbols because they are never exposed to JavaScript enumeration.
    pub fn set<K: Into<PropertyKey>>(&mut self, key: K, value: Value) {
        let key = key.into();
        if self.frozen {
            return;
        }
        if let Some((_, existing)) = self.properties.iter_mut().find(|(name, _)| name == &key) {
            *existing = Property::Value(value);
            return;
        }
        if !self.extensible {
            return;
        }
        self.insert_ordered(key, Property::Value(value));
    }

    /// Insert or overwrite an engine-private value, ignoring the ordinary object's extensibility
    /// and frozen flags. Private elements are initialized by the class machinery, not by a
    /// JavaScript property write, so `Object.freeze(this)` cannot block a later private field.
    fn set_private(&mut self, key: PropertyKey, value: Value) {
        if let Some((_, existing)) = self.properties.iter_mut().find(|(name, _)| name == &key) {
            *existing = Property::Value(value);
            return;
        }
        self.insert_ordered(key, Property::Value(value));
    }

    fn insert_ordered(&mut self, key: PropertyKey, property: Property) {
        let position = match string_index(&key) {
            Some(index) => self
                .properties
                .iter()
                .position(|(name, _)| string_index(name).is_none_or(|other| other > index))
                .unwrap_or(self.properties.len()),
            None if matches!(key, PropertyKey::String(_)) => self
                .properties
                .iter()
                .position(|(name, _)| {
                    matches!(name, PropertyKey::Symbol(_) | PropertyKey::Internal(_))
                })
                .unwrap_or(self.properties.len()),
            None => self.properties.len(),
        };
        self.properties.insert(position, (key, property));
    }

    /// Refuse every further change to this object.
    pub fn freeze(&mut self) {
        self.frozen = true;
        self.extensible = false;
    }

    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Refuse new properties, leaving the existing ones writable.
    pub fn prevent_extensions(&mut self) {
        self.extensible = false;
    }

    pub fn is_extensible(&self) -> bool {
        self.extensible
    }

    /// Remove an own property, keeping the order of the rest.
    pub fn remove<K: Into<PropertyKey>>(&mut self, key: K) {
        let key = key.into();
        if self.frozen {
            return;
        }
        self.properties.retain(|(name, _)| name != &key);
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.properties.iter().filter_map(|(name, _)| match name {
            PropertyKey::String(name) => Some(name.as_str()),
            _ => None,
        })
    }

    /// Own *enumerable* property slots in insertion order, which is the order JavaScript
    /// enumerates them in.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &Property)> {
        self.properties
            .iter()
            .filter(|(name, _)| !self.non_enumerable.iter().any(|hidden| hidden == name))
            .filter_map(|(name, property)| match name {
                PropertyKey::String(name) => Some((name.as_str(), property)),
                _ => None,
            })
    }

    /// All own keys, including symbols and VM-private slots, in insertion order.
    pub fn own_keys(&self) -> impl Iterator<Item = &PropertyKey> {
        self.properties.iter().map(|(key, _)| key)
    }

    /// Enumerable own properties in the typed key space. Symbols are included for spread and
    /// `Object.assign`, while callers implementing `Object.keys` can filter to strings.
    pub fn enumerable_entries(&self) -> impl Iterator<Item = (&PropertyKey, &Property)> {
        self.properties
            .iter()
            .filter(|(key, _)| !self.non_enumerable.iter().any(|hidden| hidden == key))
            .map(|(key, property)| (key, property))
    }

    /// All own property slots, including non-enumerable properties.
    pub fn all_entries(&self) -> impl Iterator<Item = (&PropertyKey, &Property)> {
        self.properties
            .iter()
            .map(|(key, property)| (key, property))
    }

    /// Change only the enumerable attribute of an existing property.
    pub fn set_enumerable(&mut self, key: &PropertyKey, enumerable: bool) {
        self.non_enumerable.retain(|hidden| hidden != key);
        if !enumerable && self.get_own_property(key).is_some() {
            self.non_enumerable.push(key.clone());
        }
    }

    /// Define a property that reads by name but is never enumerated, which is what every own
    /// property of an error is: `e.message` works and `Object.keys(e)` is empty.
    pub fn set_non_enumerable<K: Into<PropertyKey>>(&mut self, key: K, value: Value) {
        let key = key.into();
        self.set(key.clone(), value);
        if !self.non_enumerable.iter().any(|hidden| hidden == &key) {
            self.non_enumerable.push(key);
        }
    }
}

/// Which built-in's method table a `Native::PrototypeOf` marker stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrototypeTable {
    Object,
    Array,
    String,
    Number,
    Function,
    RegExp,
    Date,
}

/// Which element type a view reads its bytes as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedKind {
    Int8,
    Uint8,
    /// `Uint8ClampedArray`: out-of-range writes saturate instead of wrapping, which is what makes
    /// it the right type for pixel data and the wrong one for anything arithmetic.
    Uint8Clamped,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Float32,
    Float64,
}

impl TypedKind {
    /// How many bytes one element occupies.
    fn width(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 | Self::Uint8Clamped => 1,
            Self::Int16 | Self::Uint16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::Float64 => 8,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Int8 => "Int8Array",
            Self::Uint8 => "Uint8Array",
            Self::Uint8Clamped => "Uint8ClampedArray",
            Self::Int16 => "Int16Array",
            Self::Uint16 => "Uint16Array",
            Self::Int32 => "Int32Array",
            Self::Uint32 => "Uint32Array",
            Self::Float32 => "Float32Array",
            Self::Float64 => "Float64Array",
        }
    }
}

/// A typed array: a window onto a byte buffer.
#[derive(Debug)]
pub struct TypedArrayData {
    kind: TypedKind,
    /// Shared, so two views over one buffer see each other's writes.
    buffer: Rc<RefCell<Vec<u8>>>,
    /// Where the window starts, in bytes.
    offset: usize,
    /// How long the window is, in elements.
    length: usize,
}

impl PartialEq for TypedArrayData {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl TypedArrayData {
    /// The element at `index`, or `None` past the end.
    fn get(&self, index: usize) -> Option<f64> {
        if index >= self.length {
            return None;
        }
        let width = self.kind.width();
        let at = self.offset + index * width;
        let buffer = self.buffer.borrow();
        let bytes = buffer.get(at..at + width)?;
        Some(match self.kind {
            TypedKind::Int8 => f64::from(bytes[0] as i8),
            TypedKind::Uint8 | TypedKind::Uint8Clamped => f64::from(bytes[0]),
            TypedKind::Int16 => f64::from(i16::from_le_bytes([bytes[0], bytes[1]])),
            TypedKind::Uint16 => f64::from(u16::from_le_bytes([bytes[0], bytes[1]])),
            TypedKind::Int32 => {
                f64::from(i32::from_le_bytes(bytes.try_into().expect("four bytes")))
            }
            TypedKind::Uint32 => {
                f64::from(u32::from_le_bytes(bytes.try_into().expect("four bytes")))
            }
            TypedKind::Float32 => {
                f64::from(f32::from_le_bytes(bytes.try_into().expect("four bytes")))
            }
            TypedKind::Float64 => f64::from_le_bytes(bytes.try_into().expect("eight bytes")),
        })
    }

    /// Write `value` at `index`, ignoring a write past the end as the specification does.
    fn set(&self, index: usize, value: f64) {
        if index >= self.length {
            return;
        }
        let width = self.kind.width();
        let at = self.offset + index * width;
        let mut buffer = self.buffer.borrow_mut();
        let Some(slot) = buffer.get_mut(at..at + width) else {
            return;
        };
        match self.kind {
            // Every integer view but the clamped one *wraps*: the value is taken modulo 2^n,
            // which is what `ToInt32` and its siblings do and is why `u8[0] = 256` is 0.
            TypedKind::Int8 => slot[0] = to_int32(value) as u8,
            TypedKind::Uint8 => slot[0] = to_uint32(value) as u8,
            TypedKind::Uint8Clamped => {
                slot[0] = if value.is_nan() {
                    0
                } else {
                    // Saturating, and rounding halves to even, which is what the specification
                    // says and what keeps pixel arithmetic from drifting.
                    let rounded = if (value.fract().abs() - 0.5).abs() < f64::EPSILON {
                        let down = value.floor();
                        if (down as i64) % 2 == 0 {
                            down
                        } else {
                            down + 1.0
                        }
                    } else {
                        value.round()
                    };
                    rounded.clamp(0.0, 255.0) as u8
                };
            }
            TypedKind::Int16 => slot.copy_from_slice(&(to_int32(value) as i16).to_le_bytes()),
            TypedKind::Uint16 => slot.copy_from_slice(&(to_uint32(value) as u16).to_le_bytes()),
            TypedKind::Int32 => slot.copy_from_slice(&to_int32(value).to_le_bytes()),
            TypedKind::Uint32 => slot.copy_from_slice(&to_uint32(value).to_le_bytes()),
            #[allow(clippy::cast_possible_truncation)]
            TypedKind::Float32 => slot.copy_from_slice(&(value as f32).to_le_bytes()),
            TypedKind::Float64 => slot.copy_from_slice(&value.to_le_bytes()),
        }
    }

    /// Every element, as numbers.
    fn items(&self) -> Vec<Value> {
        (0..self.length)
            .map(|index| Value::Number(self.get(index).unwrap_or(0.0)))
            .collect()
    }
}

/// A proxy's two halves.
///
/// Both are held as ordinary values: a handler is a plain object whose properties are the traps,
/// and a target is whatever was proxied -- including another proxy, which is why neither is
/// narrowed to a type here.
#[derive(Debug, PartialEq)]
pub struct ProxyData {
    target: Value,
    handler: Value,
    /// `Proxy.revocable`'s second half was called.
    ///
    /// A revoked proxy is not an empty one: every operation on it is a TypeError, which is the
    /// entire point -- the capability it represented has been withdrawn, and a program holding a
    /// stale reference has to find out rather than see a plausible answer.
    revoked: std::cell::Cell<bool>,
}

/// What `bind` captured: the function to run, the receiver to run it with, and the arguments to
/// put in front of the call's own.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundFunction {
    target: Value,
    this_value: Value,
    arguments: Vec<Value>,
}

/// A generator's saved execution state.
///
/// A generator is the first thing in this VM whose frame does not run to completion, so its
/// registers live here between resumptions rather than on the frame stack. Resuming installs
/// them, running suspends back into them, and the operand stack is saved alongside because a
/// `yield` can occur part-way through an expression.
#[derive(Debug)]
pub struct GeneratorState {
    closure: Rc<Closure>,
    locals: Vec<Value>,
    /// The generator's own operand stack, relative to where it was suspended.
    stack: Vec<Value>,
    instruction: usize,
    this_value: Value,
    new_target: Value,
    arguments: Vec<Value>,
    /// Handlers registered inside the generator, held while it is suspended.
    ///
    /// Stored as offsets rather than absolute positions: a generator's operand stack is placed
    /// wherever the resuming caller's stack happens to end, so a handler recorded during one
    /// resumption points at the wrong place during the next. That is only visible when the two
    /// resumptions happen at different stack depths -- `JSON.stringify(it.return(x))` is such a
    /// case, and is what found it.
    handlers: Vec<(usize, usize, usize)>,
    /// False until the first `next`, which starts the body rather than resuming a `yield`.
    started: bool,
    done: bool,
    /// Why the body suspended: an `await` rather than a `yield`.
    ///
    /// Meaningless for a plain generator, which only ever yields, and for an async function,
    /// which only ever awaits. An async generator does both, and its driver has to know whether
    /// the consumer's step is finished or whether it is still waiting on something.
    awaiting: bool,
}

impl PartialEq for GeneratorState {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// A callable value: which function body to run, and the bindings it closed over.
#[derive(Debug)]
pub struct Closure {
    /// The program this closure's code lives in.
    ///
    /// `function` indexes *this* table, not the running VM's. A function value that crosses a
    /// module boundary -- which is what `export function` produces -- would otherwise be called
    /// against the importer's function table and find someone else's body, or nothing at all.
    pub program: Rc<Program>,
    pub function: u32,
    /// The declared name, for a console to show. Shared with every other closure over the same
    /// proto rather than copied, because a function expression in a loop makes one closure per
    /// pass and the name is the same every time.
    pub name: Option<Rc<str>>,
    /// One cell per [`kiln_ir::FunctionProto::upvalues`] entry, in the same order.
    pub upvalues: Vec<Cell>,
    /// The enclosing function's `arguments`, captured for an arrow.
    ///
    /// An arrow has no `arguments` of its own; it sees the enclosing function's, from where it
    /// was written. Captured alongside `this` because the rule is the same one.
    pub captured_arguments: Option<Vec<Value>>,
    /// `new.target` captured where the closure was written, for an arrow function.
    ///
    /// An arrow has no `new.target` of its own for the same reason it has no `this`.
    pub captured_new_target: Option<Value>,
    /// `this` captured where the closure was written, for an arrow function.
    ///
    /// An arrow has no `this` of its own: it uses the one from its defining scope, permanently.
    /// Capturing it here rather than at the call is the whole difference between an arrow and an
    /// ordinary function, and it is why `arrow` has to survive lowering.
    pub captured_this: Option<Value>,
    /// The object `new` gives instances of this function as their prototype.
    ///
    /// Lazily installed: an ordinary function never used as a constructor never allocates one.
    pub prototype: RefCell<Option<Value>>,
    /// Own properties of the function itself, which is where static methods live.
    ///
    /// A function is an object in JavaScript, and a class's statics are properties of the
    /// constructor. Without this, `class C { static f() {} }` would have nowhere to put `f`.
    pub properties: RefCell<ObjectData>,
}

/// A shared, mutable binding.
///
/// Captured variables are shared rather than copied because JavaScript closes over the binding,
/// not the value: two closures over the same `let` must observe each other's writes, and so must
/// the enclosing function after they run.
pub type Cell = Rc<RefCell<Value>>;

/// Two closures are equal when they are the same object, which is what `===` asks.
///
/// Structural equality would be wrong: two closures over different cells can have identical
/// protos and identical current values without being the same function.
impl PartialEq for Closure {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// An array: its elements, and the properties that are not elements.
///
/// Elements and properties are separate stores because they answer different questions. `length`
/// is derived from the elements and writing an index changes it; `index` and `input` on an
/// `exec` result, or anything a program assigns by name, are ordinary properties that do not.
/// Keeping one store would mean deciding which a numeric-looking key is on every access.
#[derive(Debug, Default)]
pub struct ArrayData {
    items: RefCell<Vec<Value>>,
    /// Sparse overrides for indexed-property enumerability. Most arrays never define a
    /// descriptor, so keeping only hidden indices preserves the cheap dense path.
    index_non_enumerable: RefCell<Vec<usize>>,
    /// Empty for almost every array, and allocated with it rather than on demand because an
    /// array that never gains a named property still has to answer that it has none.
    properties: RefCell<ObjectData>,
}

impl ArrayData {
    fn new(items: Vec<Value>) -> Rc<Self> {
        Rc::new(Self {
            items: RefCell::new(items),
            index_non_enumerable: RefCell::new(Vec::new()),
            properties: RefCell::new(ObjectData::default()),
        })
    }

    fn borrow(&self) -> std::cell::Ref<'_, Vec<Value>> {
        self.items.borrow()
    }

    fn borrow_mut(&self) -> std::cell::RefMut<'_, Vec<Value>> {
        self.items.borrow_mut()
    }

    fn index_enumerable(&self, index: usize) -> bool {
        !self.index_non_enumerable.borrow().contains(&index)
    }

    fn set_index_enumerable(&self, index: usize, enumerable: bool) {
        let mut hidden = self.index_non_enumerable.borrow_mut();
        hidden.retain(|at| *at != index);
        if !enumerable {
            let position = hidden
                .iter()
                .position(|at| *at > index)
                .unwrap_or(hidden.len());
            hidden.insert(position, index);
        }
    }

    fn remove_index_attributes(&self, index: usize) {
        self.index_non_enumerable
            .borrow_mut()
            .retain(|at| *at != index);
    }

    fn replace_index_attributes(&self, hidden: impl IntoIterator<Item = usize>) {
        *self.index_non_enumerable.borrow_mut() = hidden.into_iter().collect();
    }

    fn truncate_index_attributes(&self, length: usize) {
        self.index_non_enumerable
            .borrow_mut()
            .retain(|at| *at < length);
    }
}

impl PartialEq for ArrayData {
    /// Identity: `[] === []` is false, and two references to one array are equal.
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// A compiled regular expression, its source, and the cursor `g` and `y` advance.
///
/// The engine is `regress`, which targets ECMAScript semantics rather than the linear-time
/// subset: backreferences, lookaround and sticky matching are all things a JavaScript program
/// expects and a linear-time engine cannot offer. It also matches over UTF-16, so an index it
/// reports is already in the units the language counts in and needs no conversion.
#[derive(Debug)]
pub struct RegExpData {
    /// The pattern as written, which `source` hands back to the program.
    source: String,
    flags: String,
    compiled: regress::Regex,
    global: bool,
    sticky: bool,
    /// Where the next `g` or `y` search starts, in UTF-16 units. Mutable and program-visible,
    /// which is why two evaluations of one literal must not share an object.
    last_index: std::cell::Cell<usize>,
}

impl PartialEq for RegExpData {
    /// Identity, like every other object: two literals with the same source are two regexps.
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// A JavaScript scalar value represented by the interpreter.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    /// A shared binding living in a local slot, put there by `NewCell`.
    ///
    /// Never observable from JavaScript: the compiler only emits cell instructions for slots it
    /// boxed, so a cell is always read through `LoadCell`/`LoadUpvalue`, never pushed as a value.
    /// It is a variant of `Value` rather than a separate slot type so that a frame's locals stay
    /// one homogeneous vector.
    Cell(Cell),
    /// A built-in provided by the runtime rather than compiled from source.
    ///
    /// `Copy` and compared by value: there is exactly one `Error` in a realm, so two references
    /// to it are the same function, which is what `===` should say. That stops being true when
    /// built-ins move into JavaScript (design.md §5) and become ordinary objects.
    Native(Native),
    /// A suspended generator: a frame that can stop and resume, which nothing else here needs.
    Generator(Rc<RefCell<GeneratorState>>),
    Promise(Rc<RefCell<PromiseState>>),
    RegExp(Rc<RegExpData>),
    /// A date: milliseconds since the epoch, behind an `Rc` because two dates built from the same
    /// instant are still two objects and `===` has to say so.
    Date(Rc<std::cell::Cell<f64>>),
    /// An arbitrary-precision integer. A separate numeric type rather than a wider `Number`,
    /// because the language keeps them apart on purpose: mixing one with a number in arithmetic
    /// is a TypeError, and precision that does not silently stop is the reason to reach for it.
    BigInt(Rc<num_bigint::BigInt>),
    /// The `resolve` or `reject` handed to a promise executor.
    ///
    /// A function bound to one promise, which no other value kind here can express: a `Native`
    /// carries no state and a `Closure` carries bytecode. The flag says which of the two it is.
    Settler(Rc<RefCell<PromiseState>>, bool),
    /// An array. Separate from `Object` rather than an object with numeric keys: `Array.isArray`
    /// has to be able to tell them apart, `length` is not an ordinary property, and a dense
    /// `Vec` is the representation that makes indexing the common case it is.
    Array(Rc<ArrayData>),
    /// A built-in that takes a callback, invoked by the VM rather than by `call_native`.
    Callback(CallbackMethod),
    /// A view over a byte buffer: `Uint8Array` and its siblings.
    ///
    /// Separate from `Array` because the difference is the point: the elements are bytes in a
    /// shared buffer with a fixed width and a fixed length, two views can overlap, and writing
    /// through one is visible through the other. An array of numbers cannot express any of that.
    TypedArray(Rc<TypedArrayData>),
    /// An `ArrayBuffer`: bytes with no interpretation, which only a view gives them.
    ArrayBuffer(Rc<RefCell<Vec<u8>>>),
    /// A `new Proxy(target, handler)`.
    ///
    /// Its own value kind rather than an object carrying reserved state, because *every*
    /// operation on it has to consult the handler first -- a read, a write, `in`, `delete`,
    /// enumerating keys, calling it. Modelling it as an object would make each of those places
    /// have to remember to check, and forgetting one is a silently wrong answer.
    Proxy(Rc<ProxyData>),
    /// The result of `f.bind(this, ...args)`.
    ///
    /// A wrapper rather than a rewritten closure, because binding is defined on *any* callable --
    /// a native and a settler included -- and because `bind` is repeatable: binding a bound
    /// function fixes more arguments and leaves the first receiver in place, which falls out of
    /// nesting these and is awkward any other way.
    Bound(Rc<BoundFunction>),
    /// A well-known symbol. User-created symbols need a realm-wide registry and do not exist yet.
    Symbol(Symbol),
    /// An object. Shared by `Rc` because objects have identity: `a === b` asks whether two
    /// references name the same object, and a mutation through one must be visible through the
    /// other.
    Object(Rc<RefCell<ObjectData>>),
    /// A callable: a proto plus the cells it captured when it was created.
    ///
    /// Shared by `Rc` because two references to the same closure must be the same object --
    /// `f === f` -- and because a closure outlives the frame that produced it. Reference cycles
    /// through captured cells are not collected; that is a real limitation and the right fix is
    /// a collector, not a weaker representation here.
    Function(Rc<Closure>),
}

impl Value {
    /// The string a value becomes when used as a property key.
    ///
    /// Distinct from `to_display_string`: this is the historical printable symbol spelling, not
    /// the object-store key. Symbol identity is retained by [`Value::to_property_key`].
    pub fn property_key(&self) -> String {
        match self {
            Self::Symbol(symbol) => symbol.property_key(),
            other => other.to_display_string(),
        }
    }

    pub fn to_property_key(&self) -> PropertyKey {
        match self {
            Self::Symbol(symbol) => PropertyKey::Symbol(symbol.clone()),
            other => PropertyKey::String(other.property_key()),
        }
    }

    /// What `typeof` reports.
    ///
    /// Distinct from [`Value::type_name`], which is for diagnostics: `typeof null` is the string
    /// "object", a specification bug old enough to be load-bearing, while a diagnostic saying a
    /// value is an object when it is null would just be confusing.
    pub fn type_of(&self) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::Null => "object",
            Self::Boolean(_) => "boolean",
            Self::Number(_) => "number",
            Self::String(_) => "string",
            Self::Function(_) => "function",
            Self::Object(_) => "object",
            // A method table stands for a prototype *object*, and `typeof Object.prototype` is
            // "object". `Function.prototype` is the exception the specification makes: it is a
            // function that does nothing.
            Self::Native(Native::PrototypeOf(PrototypeTable::Function)) => "function",
            Self::Native(Native::PrototypeOf(_)) => "object",
            // A namespace is an object holding functions, not a function: `Math` and `JSON`
            // and `Reflect` cannot be called, and a library that feature-detects with
            // `typeof Math === "object"` has to find one.
            Self::Native(Native::Math | Native::Json | Native::Reflect) => "object",
            Self::Native(_) | Self::Callback(_) | Self::Bound(_) => "function",
            // `typeof` has no trap: a proxy reports whatever its target is, which is what makes
            // a proxied function still read as a function.
            Self::Proxy(proxy) => proxy.target.type_of(),
            Self::TypedArray(_) | Self::ArrayBuffer(_) => "object",
            Self::Array(_)
            | Self::Generator(_)
            | Self::Promise(_)
            | Self::RegExp(_)
            | Self::Date(_) => "object",
            Self::BigInt(_) => "bigint",
            // A settler is a function the program can call, hold and pass on, so it says so.
            Self::Settler(..) => "function",
            Self::Symbol(_) => "symbol",
            Self::Cell(_) => unreachable!("a cell is never used as a value"),
        }
    }

    /// The name JavaScript would use for this value's type, for diagnostics only.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::Null => "null",
            Self::Boolean(_) => "boolean",
            Self::Number(_) => "number",
            Self::String(_) => "string",
            Self::Function(_) => "function",
            Self::Object(_) => "object",
            Self::Native(Native::PrototypeOf(PrototypeTable::Function)) => "function",
            Self::Native(Native::PrototypeOf(_)) => "object",
            // A namespace is an object holding functions, not a function: `Math` and `JSON`
            // and `Reflect` cannot be called, and a library that feature-detects with
            // `typeof Math === "object"` has to find one.
            Self::Native(Native::Math | Native::Json | Native::Reflect) => "object",
            Self::Native(_) | Self::Callback(_) | Self::Bound(_) => "function",
            Self::Proxy(proxy) => proxy.target.type_name(),
            Self::TypedArray(view) => view.kind.name(),
            Self::ArrayBuffer(_) => "arraybuffer",
            Self::Array(_) => "array",
            Self::Generator(_) => "generator",
            Self::Promise(_) => "promise",
            Self::RegExp(_) => "regexp",
            Self::Date(_) => "date",
            Self::BigInt(_) => "bigint",
            Self::Settler(..) => "function",
            Self::Symbol(_) => "symbol",
            Self::Cell(_) => "cell",
        }
    }

    pub fn from_number_bits(bits: u64) -> Self {
        Self::Number(f64::from_bits(bits))
    }

    pub fn as_number(&self) -> f64 {
        match self {
            Self::Undefined => f64::NAN,
            Self::Null => 0.0,
            Self::Boolean(value) => f64::from(*value),
            Self::Number(value) => *value,
            Self::String(value) => parse_number(value),
            // `Number(fn)` is NaN in JavaScript: a function has no numeric value, and ToNumber
            // on it goes through ToPrimitive, which cannot produce one.
            Self::Function(_) => f64::NAN,
            // `Number({})` is NaN: ToPrimitive on a plain object yields "[object Object]",
            // which does not parse as a number.
            Self::Object(_)
            | Self::Native(_)
            | Self::Callback(_)
            | Self::Generator(_)
            | Self::Promise(_)
            | Self::RegExp(_)
            | Self::Settler(..)
            | Self::Bound(_) => f64::NAN,
            // Coercion is defined on the target: the traps that could change it are `get` on
            // `valueOf` and `toString`, which this pure path cannot call anyway.
            Self::Proxy(proxy) => proxy.target.as_number(),
            // `Number(new Uint8Array([5]))` is 5 and `Number(new Uint8Array([1,2]))` is NaN:
            // ToPrimitive joins, and only a one-element join parses as a number.
            Self::TypedArray(view) => parse_number(&join(&view.items(), ",")),
            Self::ArrayBuffer(_) => f64::NAN,
            // A date converts to its instant, which is what makes `end - start` a duration.
            Self::Date(time) => time.get(),
            // Only where a number is explicitly asked for; arithmetic refuses the mix.
            Self::BigInt(value) => {
                num_traits::ToPrimitive::to_f64(value.as_ref()).unwrap_or(f64::NAN)
            }
            // `Number(Symbol())` is a TypeError; NaN is the closest thing available without one.
            Self::Symbol(_) => f64::NAN,
            // `Number([])` is 0 and `Number([2])` is 2: ToPrimitive joins, then parses.
            Self::Array(items) => parse_number(&join(&items.borrow(), ",")),
            Self::Cell(_) => unreachable!("a cell is never used as a value"),
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Undefined | Self::Null => false,
            Self::Boolean(value) => *value,
            Self::Number(value) => *value != 0.0 && !value.is_nan(),
            Self::String(value) => !value.is_empty(),
            // Every object is truthy, and a function is an object.
            Self::Function(_) => true,
            // Every object is truthy, including an empty one.
            Self::Object(_)
            | Self::Native(_)
            | Self::Callback(_)
            | Self::Array(_)
            | Self::Generator(_)
            | Self::Promise(_)
            | Self::RegExp(_)
            | Self::Date(_)
            | Self::Settler(..)
            | Self::Bound(_)
            | Self::Proxy(_)
            | Self::TypedArray(_)
            | Self::ArrayBuffer(_)
            | Self::Symbol(_) => true,
            Self::BigInt(value) => !num_traits::Zero::is_zero(value.as_ref()),
            Self::Cell(_) => unreachable!("a cell is never used as a value"),
        }
    }

    pub fn is_nullish(&self) -> bool {
        matches!(self, Self::Undefined | Self::Null)
    }

    pub fn to_display_string(&self) -> String {
        match self {
            Self::Undefined => "undefined".to_owned(),
            Self::Null => "null".to_owned(),
            Self::Boolean(value) => value.to_string(),
            Self::Number(value) => number_string(*value),
            Self::String(value) => value.clone(),
            // Not `Function.prototype.toString`, which returns the source text. Producing that
            // needs the span and the original source, neither of which reaches the VM; naming
            // the function is more useful here than an invented body would be.
            // Not `Function.prototype.toString`, which returns source text the VM never sees.
            // The proto index is stable and useful when reading a trace; a made-up body is not.
            Self::Function(closure) => format!("[Function {}]", closure.function),
            // What `String({})` produces. `console.log` in a real console prints the contents
            // instead, which needs a formatter the host side does not have yet.
            Self::Object(_) => "[object Object]".to_owned(),
            // The same name a console shows. `String(f)` in node is the source text, which a
            // backend does not have; naming the function beats an invented body, and naming it
            // the way the program does beats naming it the way the implementation does.
            Self::Native(native) => match namespace_name(native) {
                // A namespace is an object that happens to hold functions, and `String` of an
                // object is its `toString` tag. `typeof Math` already answered `"object"`; this
                // is the same fact reaching the other conversion.
                Some(name) => format!("[object {name}]"),
                None => format!("[Function: {}]", native_js_name(native)),
            },
            Self::Callback(method) => {
                format!("[Function: {}]", builtin_js_name(&format!("{method:?}")))
            }
            Self::Generator(_) => "[object Generator]".to_owned(),
            Self::Promise(_) => "[object Promise]".to_owned(),
            // `String(/a/g)` is the literal as written, which is what `toString` returns.
            Self::RegExp(regexp) => format!("/{}/{}", regexp.source, regexp.flags),
            Self::Date(time) => date_to_string(time.get()),
            // No `n` suffix: that is the literal's spelling, not the value's.
            Self::BigInt(value) => value.to_string(),
            Self::Settler(..) | Self::Bound(_) => "function () { [native code] }".to_owned(),
            Self::Proxy(proxy) => proxy.target.to_display_string(),
            // Like an array: `String(new Uint8Array([1, 2]))` is "1,2".
            Self::TypedArray(view) => join(&view.items(), ","),
            Self::ArrayBuffer(_) => "[object ArrayBuffer]".to_owned(),
            Self::Symbol(symbol) => format!("Symbol({})", symbol.description()),
            // `String([1,2])` is "1,2": arrays stringify as a comma join, not as [object Array].
            Self::Array(items) => join(&items.borrow(), ","),
            Self::Cell(_) => unreachable!("a cell is never used as a value"),
        }
    }
}

impl From<Constant> for Value {
    fn from(value: Constant) -> Self {
        match value {
            Constant::Undefined => Self::Undefined,
            Constant::Null => Self::Null,
            Constant::Boolean(value) => Self::Boolean(value),
            Constant::Number(bits) => Self::from_number_bits(bits),
            Constant::String(value) => Self::String(value),
            // Parsed once per push rather than held pre-parsed on the program, which would put
            // a heap value in a table the cache hashes by bytes.
            Constant::BigInt(digits) => {
                Self::BigInt(Rc::new(digits.parse().expect(
                    "a BigInt constant holds digits the frontend already parsed",
                )))
            }
        }
    }
}

impl From<Value> for HostValue {
    fn from(value: Value) -> Self {
        match value {
            Value::Undefined => Self::Undefined,
            Value::Null => Self::Null,
            Value::Boolean(value) => Self::Boolean(value),
            Value::Number(value) => Self::Number(value.to_bits()),
            Value::String(value) => Self::String(value),
            // The ABI is scalars only, deliberately: a host operation that received a callable
            // would need the VM to call back into itself, which is a different contract from the
            // one `HostOps` has. Everything else is inspected on the way out, so `console.log`
            // shows an object's contents rather than the useless `[object Object]` that
            // `String(value)` produces -- the two really are different questions.
            other => Self::String(inspect(&other)),
        }
    }
}

impl From<HostValue> for Value {
    fn from(value: HostValue) -> Self {
        match value {
            HostValue::Undefined => Self::Undefined,
            HostValue::Null => Self::Null,
            HostValue::Boolean(value) => Self::Boolean(value),
            HostValue::Number(bits) => Self::from_number_bits(bits),
            HostValue::String(value) => Self::String(value),
        }
    }
}

/// Limits that make execution reproducible and prevent an accidental infinite loop from hanging
/// the daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionLimits {
    /// How deep a built-in may call back into JavaScript before the machine refuses.
    ///
    /// Separate from `max_call_depth` because it bounds a different resource: this one is the
    /// Rust stack, which cannot be grown and whose exhaustion is not catchable.
    pub max_native_depth: usize,
    pub max_steps: u64,
    /// Frames allowed on the call stack.
    ///
    /// A separate bound from `max_steps` because unbounded recursion exhausts the host stack
    /// long before it exhausts a step budget, and the failure mode is a process abort rather
    /// than a diagnostic. Bounding frames turns it into an error the caller can report.
    pub max_call_depth: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_steps: 1_000_000,
            // Roughly what node allows. A JavaScript frame is a `Frame` in a vector rather than
            // a Rust stack frame, so ordinary recursion costs heap and not stack; 512 refused
            // programs node runs.
            max_call_depth: 10_000,
            // The interpreter *does* recurse in Rust when a built-in calls back into JavaScript
            // -- `map` running a callback that calls `map` -- and that recursion is bounded by
            // the real stack rather than by a vector. Overflowing it aborts the process, which
            // is the one failure a compiler must never have, so it is bounded well below.
            max_native_depth: 192,
        }
    }
}

/// The successful result of one VM invocation.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionResult {
    pub value: Value,
    pub steps: u64,
}

/// State shared by every module evaluated in one graph realm.
///
/// Module code and call frames remain on each [`Vm`], but globals, built-in identity, and the
/// scheduler belong to the realm. This is what lets a graph behave like one JavaScript program
/// without flattening the module programs that exported closures carry with them.
/// A realm should be discarded after an execution error; its queued work and pending diagnostics
/// are part of that failed execution and must not leak into a later one.
#[derive(Clone)]
pub struct Realm {
    state: Rc<RealmState>,
}

struct RealmState {
    error_prototypes: RefCell<[Option<Value>; ErrorKind::ALL.len()]>,
    global_object: RefCell<Option<Value>>,
    process_object: RefCell<Option<Value>>,
    now: RefCell<f64>,
    timers: RefCell<Vec<Timer>>,
    microtasks: RefCell<VecDeque<Job>>,
    pending_unhandled: RefCell<Vec<(Rc<RefCell<PromiseState>>, Value)>>,
    symbol_registry: RefCell<Vec<(String, Value)>>,
    next_timer_id: RefCell<f64>,
    random_state: RefCell<u64>,
    next_sequence: RefCell<u64>,
    symbol_serial: RefCell<u64>,
    driver_program: RefCell<Option<Rc<Program>>>,
    /// Private elements are keyed by object identity rather than by the ordinary property store.
    /// Keeping the slots here gives every object-like value one uniform storage path, including
    /// arrays, proxies, typed arrays and host-backed objects that do not carry ObjectData.
    private_slots: RefCell<Vec<(Value, ObjectData)>>,
}

impl Realm {
    /// Create a fresh realm, as a standalone run or a new graph execution.
    pub fn new() -> Self {
        Self {
            state: Rc::new(RealmState {
                error_prototypes: RefCell::new([const { None }; ErrorKind::ALL.len()]),
                global_object: RefCell::new(None),
                process_object: RefCell::new(None),
                now: RefCell::new(0.0),
                timers: RefCell::new(Vec::new()),
                microtasks: RefCell::new(VecDeque::new()),
                pending_unhandled: RefCell::new(Vec::new()),
                symbol_registry: RefCell::new(Vec::new()),
                next_timer_id: RefCell::new(1.0),
                random_state: RefCell::new(RANDOM_SEED),
                next_sequence: RefCell::new(0),
                symbol_serial: RefCell::new(0),
                driver_program: RefCell::new(None),
                private_slots: RefCell::new(Vec::new()),
            }),
        }
    }

    fn remember_program(&self, program: Rc<Program>) {
        let mut driver = self.state.driver_program.borrow_mut();
        if driver.is_none() {
            *driver = Some(program);
        }
    }
}

impl Default for Realm {
    fn default() -> Self {
        Self::new()
    }
}

/// One value published by a completed module evaluation.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleExportValue {
    pub name: String,
    pub value: Value,
}

/// A completed module invocation, including the values its local export slots hold.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleExecutionResult {
    pub execution: ExecutionResult,
    pub exports: Vec<ModuleExportValue>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExecutionError {
    ImportCountMismatch {
        expected: usize,
        actual: usize,
    },
    StepLimitExceeded {
        steps: u64,
        limit: u64,
    },
    InvalidInstructionPointer {
        instruction: usize,
    },
    ConstantOutOfBounds {
        index: usize,
    },
    LocalOutOfBounds {
        index: usize,
    },
    StackUnderflow {
        instruction: usize,
    },
    InvalidJump {
        instruction: usize,
        target: usize,
    },
    Host {
        instruction: usize,
        message: String,
    },
    /// A `MakeFunction` or `Call` naming a proto the program does not contain.
    UnknownFunction {
        function: u32,
    },
    /// A property name that is not a string constant, which the compiler never emits.
    InvalidPropertyKey {
        instruction: usize,
        value: &'static str,
    },
    /// `new` on something that is not a constructor.
    NotConstructable {
        instruction: usize,
        value: &'static str,
    },
    /// Arithmetic mixing a BigInt with a number.
    ///
    /// A catchable `TypeError` in the specification, and refused rather than widened because
    /// silently converting either way loses exactly what a BigInt is for.
    MixedBigInt,
    /// A radix or digit count outside the range its method accepts.
    ///
    /// A catchable `RangeError` in the specification; fatal here for the same reason
    /// [`Self::CyclicJson`] is.
    RadixOutOfRange,
    /// `reduce` on an empty array with no initial value.
    ///
    /// A catchable `TypeError` in the specification; fatal here for the same reason
    /// [`Self::CyclicJson`] is.
    EmptyReduce,
    /// `JSON.parse` was given text that is not JSON.
    ///
    /// A catchable `SyntaxError` in the specification, and fatal here for the same reason
    /// [`Self::CyclicJson`] is: a native cannot yet raise a value the program can catch.
    InvalidJson,
    /// A pattern the regexp engine will not take.
    ///
    /// Catchable, because a program can build one out of user input -- and the frontend cannot
    /// refuse it, because `new RegExp(text)` has no text to look at until it runs.
    InvalidRegExp,
    /// `JSON.stringify` reached a value that contains itself.
    ///
    /// The specification throws a catchable `TypeError` here. A native cannot yet raise a value
    /// the program can catch, so this is fatal instead -- wrong in a way that is loud and easy
    /// to fix later, rather than quiet and wrong now.
    CyclicJson,
    /// A `.return()` signal finished unwinding a generator. Never surfaces to a caller: it is
    /// caught by the `return` that raised it.
    GeneratorReturned,
    /// An exception that reached the top of the program.
    Uncaught {
        instruction: usize,
        message: String,
        /// The functions that were on the stack, innermost first.
        ///
        /// Not `Error.prototype.stack`, which is a property the program can read and which this
        /// deliberately is not: there are no line numbers to put in one, and inventing a
        /// node-shaped string that disagrees with node's would be worse than having none. This
        /// is for the human reading the failure, and it is the difference between "somewhere in
        /// six thousand lines" and a name to grep for.
        trace: Vec<String>,
    },
    /// An exception reached a handler the *host* installed rather than one the program wrote.
    ///
    /// Never escapes the call that installed it: [`Vm::call_catching`] turns it back into the
    /// thrown value. It exists because a rejection carries a value and an `ExecutionError`
    /// carries a message, and a promise needs the value.
    CaughtByHost,
    /// A throw whose handler is below the current Rust re-entry.
    ///
    /// The value rides in [`Vm::caught`], like `CaughtByHost`, and for the same reason. It is
    /// distinct because the two want opposite things: `CaughtByHost` means a built-in asked for
    /// the value and will keep it, and this means nobody has taken it yet and the *same*
    /// handler must still receive it once the Rust stack is flat.
    ThrownAcrossNative,
    /// Any operation on a proxy whose `revoke` has been called.
    RevokedProxy,
    /// A proxy nested inside a value being serialised.
    ProxyNotSerialisable,
    /// `array.length = x` where `x` is not a whole number in range.
    InvalidArrayLength,
    /// A percent-escape that is not two hex digits, or that decodes to invalid UTF-8.
    MalformedUri,
    /// `push` and its siblings on a frozen array.
    FrozenArray,
    /// `new Function(source)`, which needs a compiler at run time.
    FunctionFromSource,
    /// A property assignment on something that is not an object.
    NotAnObject {
        instruction: usize,
        value: &'static str,
    },
    /// A capture descriptor pointing past the creating closure's upvalues.
    MissingUpvalue {
        index: u32,
    },
    /// A capture or cell access on a slot that was never boxed.
    NotACell {
        slot: usize,
    },
    NotCallable {
        instruction: usize,
        value: &'static str,
    },
    CallDepthExceeded {
        limit: usize,
    },
    GlobalOutOfBounds {
        index: usize,
    },
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ImportCountMismatch { expected, actual } => write!(
                formatter,
                "module expected {expected} imported values but received {actual}"
            ),
            Self::StepLimitExceeded { steps, limit } => {
                write!(
                    formatter,
                    "execution exceeded {limit} steps after {steps} steps"
                )
            }
            Self::InvalidInstructionPointer { instruction } => {
                write!(formatter, "invalid instruction pointer {instruction}")
            }
            Self::ConstantOutOfBounds { index } => write!(formatter, "constant {index} is missing"),
            Self::LocalOutOfBounds { index } => write!(formatter, "local {index} is missing"),
            Self::StackUnderflow { instruction } => {
                write!(formatter, "stack underflow at instruction {instruction}")
            }
            Self::InvalidJump {
                instruction,
                target,
            } => {
                write!(
                    formatter,
                    "invalid jump from instruction {instruction} to {target}"
                )
            }
            Self::InvalidPropertyKey { instruction, value } => write!(
                formatter,
                "instruction {instruction} used a {value} as a property name"
            ),
            Self::NotConstructable { instruction, value } => write!(
                formatter,
                "instruction {instruction} used `new` on a {value}, which is not a constructor"
            ),
            Self::MixedBigInt => write!(
                formatter,
                "cannot mix BigInt and other types, use explicit conversions"
            ),
            Self::RadixOutOfRange => {
                write!(
                    formatter,
                    "argument is out of the range this method accepts"
                )
            }
            Self::EmptyReduce => write!(formatter, "reduce of empty array with no initial value"),
            Self::InvalidJson => write!(formatter, "unexpected token in JSON"),
            Self::InvalidRegExp => write!(formatter, "invalid regular expression"),
            Self::RevokedProxy => write!(
                formatter,
                "cannot perform this operation on a revoked proxy"
            ),
            Self::InvalidArrayLength => write!(formatter, "invalid array length"),
            Self::MalformedUri => write!(formatter, "URI malformed"),
            Self::FrozenArray => write!(formatter, "cannot change the length of a frozen array"),
            Self::FunctionFromSource => write!(
                formatter,
                "`Function` cannot build a function from source at run time"
            ),
            Self::ProxyNotSerialisable => {
                write!(formatter, "cannot serialise a proxy nested inside a value")
            }
            Self::CyclicJson => write!(formatter, "converting circular structure to JSON"),
            Self::GeneratorReturned => write!(formatter, "generator returned"),
            Self::Uncaught { message, trace, .. } => {
                write!(formatter, "uncaught {message}")?;
                for frame in trace {
                    write!(formatter, "\n    at {frame}")?;
                }
                Ok(())
            }
            Self::CaughtByHost => write!(formatter, "exception delivered to the host"),
            Self::ThrownAcrossNative => {
                write!(formatter, "exception travelling out of a built-in callback")
            }
            Self::NotAnObject { instruction, value } => write!(
                formatter,
                "instruction {instruction} set a property on a {value}, which is not an object"
            ),
            Self::MissingUpvalue { index } => {
                write!(formatter, "no captured cell at upvalue index {index}")
            }
            Self::NotACell { slot } => {
                write!(formatter, "local slot {slot} does not hold a captured cell")
            }
            Self::UnknownFunction { function } => {
                write!(formatter, "no compiled function at index {function}")
            }
            Self::NotCallable { instruction, value } => write!(
                formatter,
                "instruction {instruction} tried to call a {value}, which is not a function"
            ),
            Self::CallDepthExceeded { limit } => {
                write!(formatter, "call depth exceeded {limit} frames")
            }
            Self::GlobalOutOfBounds { index } => {
                write!(formatter, "module slot {index} is out of bounds")
            }
            Self::Host {
                instruction,
                message,
            } => {
                write!(
                    formatter,
                    "host operation failed at instruction {instruction}: {message}"
                )
            }
        }
    }
}

impl ExecutionError {
    /// The JavaScript error this corresponds to, when it is one a program could catch.
    ///
    /// Everything else is a compiler defect or a resource limit -- a program cannot meaningfully
    /// catch "bytecode jumped out of range", and letting it try would turn a bug in kiln into an
    /// exception the program silently swallows. So the list is short on purpose.
    fn as_javascript_error(&self) -> Option<(ErrorKind, String)> {
        Some(match self {
            Self::NotCallable { value, .. } => {
                (ErrorKind::Type, format!("a {value} is not a function"))
            }
            Self::NotConstructable { value, .. } => {
                (ErrorKind::Type, format!("a {value} is not a constructor"))
            }
            Self::NotAnObject { value, .. } => (
                ErrorKind::Type,
                format!("cannot set a property on a {value}"),
            ),
            Self::RevokedProxy => (
                ErrorKind::Type,
                "Cannot perform this operation on a revoked proxy".to_owned(),
            ),
            Self::InvalidArrayLength => (ErrorKind::Range, "Invalid array length".to_owned()),
            // What a real engine raises when the stack runs out, and catchable for the same
            // reason: a program that recurses on user input has to be able to say so.
            Self::CallDepthExceeded { .. } => (
                ErrorKind::Range,
                "Maximum call stack size exceeded".to_owned(),
            ),
            // The specification has a `URIError` for this; this realm's nearest kind is used
            // rather than adding one nothing else would ever raise.
            Self::MalformedUri => (ErrorKind::Type, "URI malformed".to_owned()),
            Self::FrozenArray => (
                ErrorKind::Type,
                "Cannot add property, object is not extensible".to_owned(),
            ),
            Self::FunctionFromSource => (
                ErrorKind::Type,
                "Function cannot be built from source at run time".to_owned(),
            ),
            Self::ProxyNotSerialisable => (
                ErrorKind::Type,
                "Cannot serialise a proxy nested inside a value".to_owned(),
            ),
            Self::CyclicJson => (
                ErrorKind::Type,
                "Converting circular structure to JSON".to_owned(),
            ),
            Self::InvalidJson => (ErrorKind::Syntax, "Unexpected token in JSON".to_owned()),
            Self::MixedBigInt => (
                ErrorKind::Type,
                "Cannot mix BigInt and other types, use explicit conversions".to_owned(),
            ),
            Self::RadixOutOfRange => (
                ErrorKind::Range,
                "toString() radix must be between 2 and 36".to_owned(),
            ),
            Self::InvalidRegExp => (ErrorKind::Syntax, "Invalid regular expression".to_owned()),
            Self::EmptyReduce => (
                ErrorKind::Type,
                "Reduce of empty array with no initial value".to_owned(),
            ),
            _ => return None,
        })
    }
}

impl std::error::Error for ExecutionError {}

/// Execute one IR program against a host implementation.
pub fn execute<H: HostOps + ?Sized>(
    program: Rc<Program>,
    host: &mut H,
    limits: ExecutionLimits,
) -> Result<ExecutionResult, ExecutionError> {
    let realm = Realm::new();
    let mut vm = Vm::new(
        Rc::clone(&program),
        vec![Value::Undefined; program.local_count as usize],
        host,
        limits,
        &realm,
    );
    // The program is not finished when its last statement is: anything it scheduled still has
    // to run, which is what makes a `setTimeout` observable at all.
    let execution = vm.run()?;
    vm.run_event_loop()?;
    Ok(ExecutionResult {
        steps: vm.steps,
        ..execution
    })
}

/// The object `import * as ns` binds: one property per export of the provider.
///
/// Built by the linker rather than by the module itself, because the module does not know it is
/// being imported this way. Frozen, which is what a module namespace is -- assigning to one is a
/// TypeError in the specification and silently ignored here, as every frozen object's writes are.
pub fn namespace_object(exports: impl IntoIterator<Item = (String, Value)>) -> Value {
    let mut data = ObjectData::default();
    for (name, value) in exports {
        // A namespace object holds values, so an exported cell is read through here. Node's
        // namespace objects are live; this one is a snapshot taken when the provider finished.
        data.set(&name, read_through_cell(&value));
    }
    data.freeze();
    Value::Object(Rc::new(RefCell::new(data)))
}

/// Whether this generator object came from an `async function*`.
fn is_async_generator(state: &Rc<RefCell<GeneratorState>>) -> bool {
    let state = state.borrow();
    state
        .closure
        .program
        .functions
        .get(state.closure.function as usize)
        .is_some_and(|proto| proto.async_generator)
}

/// What a value is, looking through an exported cell.
///
/// An export is a cell so that a named import is a live view of it. Everywhere a *value* is
/// wanted instead -- a namespace object, a star expansion -- goes through this.
pub fn read_through_cell(value: &Value) -> Value {
    match value {
        Value::Cell(cell) => cell.borrow().clone(),
        other => other.clone(),
    }
}

/// The own properties of a namespace object, in the order it holds them.
///
/// The inverse of [`namespace_object`], for the linker expanding a `export * from` entry back
/// into the names it stands for.
pub fn namespace_entries(value: &Value) -> Vec<(String, Value)> {
    let Value::Object(object) = value else {
        return Vec::new();
    };
    object
        .borrow()
        .entries()
        .filter_map(|(name, property)| match property {
            Property::Value(value) => Some((name.to_owned(), value.clone())),
            Property::Accessor { .. } => None,
        })
        .collect()
}

/// Execute one linkage-aware module after its acyclic dependencies have completed.
///
/// `import_values` is ordered exactly like [`ModuleProgram::imports`]. Each value is installed in
/// the compiler-assigned local slot before the first bytecode instruction executes.
pub fn execute_module<H: HostOps + ?Sized>(
    module: &ModuleProgram,
    import_values: &[Value],
    host: &mut H,
    limits: ExecutionLimits,
) -> Result<ModuleExecutionResult, ExecutionError> {
    execute_module_with_realm(module, import_values, host, limits, &Realm::new(), true)
}

/// Execute one module body in an existing realm without draining its scheduler.
///
/// Graph execution uses this to evaluate every synchronous module body first. The caller drains
/// the realm once, after the dependency-first body order is complete.
pub fn execute_module_in_realm<H: HostOps + ?Sized>(
    module: &ModuleProgram,
    import_values: &[Value],
    host: &mut H,
    limits: ExecutionLimits,
    realm: &Realm,
) -> Result<ModuleExecutionResult, ExecutionError> {
    execute_module_with_realm(module, import_values, host, limits, realm, false)
}

/// Drain the shared realm scheduler after all synchronous graph module bodies have run.
pub fn drain_realm<H: HostOps + ?Sized>(
    realm: &Realm,
    host: &mut H,
    limits: ExecutionLimits,
) -> Result<u64, ExecutionError> {
    let program = realm
        .state
        .driver_program
        .borrow()
        .clone()
        .unwrap_or_else(|| {
            Rc::new(Program {
                version: kiln_ir::BYTECODE_VERSION,
                constants: Vec::new(),
                code: vec![Instruction::Return],
                local_count: 0,
                functions: Vec::new(),
            })
        });
    let mut vm = Vm::new(program, Vec::new(), host, limits, realm);
    vm.run_event_loop()?;
    Ok(vm.steps)
}

fn execute_module_with_realm<H: HostOps + ?Sized>(
    module: &ModuleProgram,
    import_values: &[Value],
    host: &mut H,
    limits: ExecutionLimits,
    realm: &Realm,
    drain: bool,
) -> Result<ModuleExecutionResult, ExecutionError> {
    if import_values.len() != module.imports.len() {
        return Err(ExecutionError::ImportCountMismatch {
            expected: module.imports.len(),
            actual: import_values.len(),
        });
    }

    let mut locals = vec![Value::Undefined; module.program.local_count as usize];
    for (binding, value) in module.imports.iter().zip(import_values) {
        let Some(local) = locals.get_mut(binding.local_slot as usize) else {
            return Err(ExecutionError::LocalOutOfBounds {
                index: binding.local_slot as usize,
            });
        };
        // An import slot always holds a cell, because an import is a live view of the
        // exporter's binding. When the linker hands over the exporter's own cell that view is
        // genuinely live; when it hands over a plain value -- a re-export, a star-expanded name,
        // a namespace object -- it is wrapped here and reads as a snapshot, which is what those
        // forms are until the linker learns to thread cells through them too.
        *local = match value {
            Value::Cell(_) => value.clone(),
            other => Value::Cell(Rc::new(RefCell::new(other.clone()))),
        };
    }

    let mut vm = Vm::new(Rc::new(module.program.clone()), locals, host, limits, realm);
    let execution = vm.run()?;
    // Exports are read before the event loop runs: they are the module's interface, and a timer
    // that mutates a binding afterwards has not changed what the module exported.
    if drain {
        vm.run_event_loop()?;
    }
    let execution = ExecutionResult {
        steps: vm.steps,
        ..execution
    };
    let mut exports = Vec::with_capacity(module.exports.len());
    for binding in &module.exports {
        let Some(value) = vm.locals.get(binding.local_slot as usize) else {
            return Err(ExecutionError::LocalOutOfBounds {
                index: binding.local_slot as usize,
            });
        };
        // The cell itself, not what is in it: the importer holds this module's binding rather
        // than a copy of the value it had when the module finished. `namespace_object` and the
        // star expansion read through it, because those *are* snapshots.
        exports.push(ModuleExportValue {
            name: binding.exported.clone(),
            value: value.clone(),
        });
    }
    Ok(ModuleExecutionResult { execution, exports })
}

/// A callback scheduled to run at a point on the virtual clock.
///
/// Time here is a number the machine controls rather than a reading of the host's clock, which is
/// what design.md §11 asks for: no sleeping, exact ordering, and the same answer every run.
#[derive(Debug)]
struct Timer {
    /// What `clearTimeout` is given back. A number, because that is what the language says.
    id: f64,
    /// The virtual instant this is due at.
    due: f64,
    /// Breaks ties between timers due at the same instant, so two `setTimeout(f, 0)` calls run in
    /// the order they were made rather than in whatever order the list happens to hold them.
    sequence: u64,
    callback: Value,
    arguments: Vec<Value>,
    /// `Some(delay)` for `setInterval`, which reschedules itself after each run.
    repeat: Option<f64>,
}

/// A promise: a value that is not there yet, and the reactions waiting for it.
///
/// Its own value kind rather than an object with reserved keys, unlike Set and Map, because the
/// waiting reactions are machine state rather than program-visible properties -- putting them in
/// an object would mean a spread or a `JSON.stringify` could reach them.
#[derive(Debug, PartialEq)]
pub struct PromiseState {
    settled: Option<Settled>,
    /// Reactions registered while still pending, in the order `then` was called.
    reactions: Vec<Reaction>,
    /// Whether a rejection has somewhere to go. A promise rejected with nothing attached is a
    /// silent failure, which is worth reporting rather than dropping.
    handled: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Settled {
    Fulfilled(Value),
    Rejected(Value),
}

/// One `then` waiting on a promise: what to run, and where the result goes.
#[derive(Debug, PartialEq)]
struct Reaction {
    on_fulfilled: Value,
    on_rejected: Value,
    sink: Sink,
    /// `finally`: run the handler, then settle the sink with the *original* outcome rather than
    /// the handler's result. That is the whole difference from `then(f, f)`, which would replace
    /// a rejection with a fulfilment.
    passthrough: bool,
}

/// Where a settled outcome goes.
#[derive(Debug, PartialEq)]
enum Sink {
    /// The promise `then` returned.
    Promise(Rc<RefCell<PromiseState>>),
    /// One input of a combinator. `Promise.all` and its siblings have to count and collect, which
    /// is bookkeeping no JavaScript handler performs -- so their inputs feed this instead.
    Combinator(Native, usize, Rc<Combinator>),
    /// Settle a promise with a *recorded* outcome once something else has settled, unless that
    /// something rejected. `finally` waits on its handler's result and then restores the outcome
    /// it was passing through.
    Replay(Rc<RefCell<PromiseState>>, Settled),
    /// An `await` inside an async function: resume its body with whatever the awaited promise
    /// produced, and settle the function's own promise when the body finishes.
    Async(Rc<RefCell<GeneratorState>>, Rc<RefCell<PromiseState>>),
    /// An `await` inside an async *generator*: resume its body, and settle the promise the
    /// consumer's `next()` returned once the body reaches its next `yield` or finishes.
    ///
    /// The promise here belongs to one step rather than to the whole function, which is the
    /// whole difference between the two.
    AsyncGenerator(Rc<RefCell<GeneratorState>>, Rc<RefCell<PromiseState>>),
}

/// The shared state of one `Promise.all`, `allSettled`, `race` or `any`.
///
/// On the heap because each input settles later and has to find it, and behind `Cell`s because
/// several reactions reach it and none of them holds it for longer than a step.
#[derive(Debug, PartialEq)]
struct Combinator {
    /// One slot per input, filled as they settle -- so the result keeps the *input* order rather
    /// than the order they finished in.
    slots: RefCell<Vec<Value>>,
    outstanding: std::cell::Cell<usize>,
    result: Rc<RefCell<PromiseState>>,
}

impl PromiseState {
    fn pending() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self {
            settled: None,
            reactions: Vec::new(),
            handled: false,
        }))
    }
}

/// One unit of work on the microtask queue.
///
/// Both kinds drain together and in order, because the language puts them on one queue: a
/// `queueMicrotask` between two `then`s runs between them.
#[derive(Debug)]
enum Job {
    /// `queueMicrotask(callback)`.
    Callback(Value, Vec<Value>),
    /// A settled promise's reaction, waiting to be run.
    React(Settled, Reaction),
    /// Attach `outer` to `inner`, for a promise resolved with another promise.
    ///
    /// A job rather than an immediate attachment because the specification resolves a thenable by
    /// *queueing* a call to its `then`, which costs a tick -- so returning a promise from a
    /// handler settles one turn later than returning a plain value. That is observable, and
    /// programs order themselves around it.
    Adopt(Rc<RefCell<PromiseState>>, Rc<RefCell<PromiseState>>),
}

/// How a suspended generator is resumed.
///
/// One enum rather than a pair of flags, because the three are mutually exclusive and each is a
/// different thing to do at the suspension point: hand a value back, raise one there, or unwind.
#[derive(Clone, Debug)]
enum Resume {
    /// `next(v)`: the value becomes the result of the `yield`.
    Send(Value),
    /// `throw(e)`, and an `await` whose promise rejected. Raised *at* the suspension point, so
    /// the generator's own `try` blocks see it.
    Throw(Value),
    /// `return(v)`: unwinds through every `finally` without resuming the `yield` at all.
    Return(Value),
}

/// The `target` of a handler the host installed, which no compiler-emitted handler can have.
const HOST_HANDLER: usize = usize::MAX;

/// A `catch` or `finally` waiting for an exception.
///
/// The frame depth and stack height are recorded so unwinding can restore the machine to exactly
/// the state the handler expects, however many frames the throw crossed.
#[derive(Debug)]
struct Handler {
    target: usize,
    frame_depth: usize,
    stack_height: usize,
    /// How many Rust re-entries deep the machine was when this was installed.
    ///
    /// A built-in that takes a callback -- `map`, `sort`, a promise reaction -- drives the
    /// interpreter from inside a Rust frame of its own. A throw from that callback cannot jump
    /// straight to a handler installed *below* that frame: the Rust driver would still be
    /// sitting there, and would carry on running whatever code the jump landed on. So the
    /// depths are compared, and a throw that would cross one travels as an error instead until
    /// the Rust stack is flat again.
    native_depth: usize,
}

/// A suspended caller, restored when the callee returns.
///
/// The module body is not represented here: it is the bottom of the call stack rather than a
/// frame in it, which is why an empty `frames` means `Return` ends execution.
struct Frame {
    /// The program that was running, restored with the frame.
    program: Rc<Program>,
    /// `new.target` in the caller, restored with the frame.
    new_target: Value,
    /// The closure that was running, or `None` for the module body.
    function: Option<Rc<Closure>>,
    this_value: Value,
    /// What this frame was called with, so `super(...arguments)` can forward them.
    arguments: Vec<Value>,
    /// The instance being built, when this frame is a `new` call.
    ///
    /// Held here rather than in a parallel stack so that `Return` can decide locally: a
    /// constructor yields its instance unless the body returned an object, which wins.
    constructing: Option<Value>,
    locals: Vec<Value>,
    /// Where to resume in the caller.
    instruction: usize,
    /// Stack height when the call began, so a callee that leaves debris cannot corrupt the
    /// caller's operand stack.
    stack_base: usize,
}

struct Vm<'host, H: HostOps + ?Sized> {
    /// The program the *running* frame belongs to, swapped on every call and restored on return.
    program: Rc<Program>,
    host: &'host mut H,
    stack: Vec<Value>,
    locals: Vec<Value>,
    /// The closure currently executing: `None` is the module body.
    function: Option<Rc<Closure>>,
    /// Set by `Yield`, so the run loop knows it stopped at a suspension rather than a return.
    suspended: bool,
    /// Active exception handlers, innermost last.
    handlers: Vec<Handler>,
    /// The arguments the running frame received.
    arguments: Vec<Value>,
    /// `this` in the running frame. At module scope it is `undefined`, because a module body is
    /// always strict and strict code does not get the global object.
    this_value: Value,
    /// Realm state shared by all module VMs in one graph execution.
    realm: Rc<RealmState>,
    /// How many built-ins are currently between the running code and the interpreter loop.
    native_depth: usize,
    /// Whether the suspension that just happened was an `await` rather than a `yield`.
    ///
    /// Written where the body stops and read by whoever resumed it, because the instruction that
    /// stopped is not otherwise visible from there.
    suspended_awaiting: bool,
    /// `new.target` in the running frame: the constructor a `new` call named, or `undefined`.
    ///
    /// Held beside `this` and saved with it, because it follows exactly the same rule -- an
    /// ordinary call has none, and an arrow uses the one from where it was written.
    new_target: Value,
    frames: Vec<Frame>,
    instruction: usize,
    /// The value delivered to a host handler, waiting to be collected by `call_catching`.
    caught: Option<Value>,
    steps: u64,
    limits: ExecutionLimits,
}

/// Whether a value can carry ECMAScript internal slots. Primitive values cannot be branded, but
/// every object-like representation can be queried by a private instruction. The private store
/// is separate from ordinary properties because several host-backed values have no ObjectData.
fn private_target(value: &Value) -> bool {
    matches!(
        value,
        Value::Object(_)
            | Value::Function(_)
            | Value::Array(_)
            | Value::Generator(_)
            | Value::Promise(_)
            | Value::RegExp(_)
            | Value::Date(_)
            | Value::Settler(..)
            | Value::TypedArray(_)
            | Value::ArrayBuffer(_)
            | Value::Proxy(_)
            | Value::Bound(_)
            | Value::Native(_)
            | Value::Callback(_)
    )
}

/// Identity comparison for values that can carry private elements. The realm keeps one strong
/// reference to each branded object, preventing pointer reuse from aliasing a later object.
fn same_private_target(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Object(a), Value::Object(b)) => Rc::ptr_eq(a, b),
        (Value::Function(a), Value::Function(b)) => Rc::ptr_eq(a, b),
        (Value::Array(a), Value::Array(b)) => Rc::ptr_eq(a, b),
        (Value::Generator(a), Value::Generator(b)) => Rc::ptr_eq(a, b),
        (Value::Promise(a), Value::Promise(b)) => Rc::ptr_eq(a, b),
        (Value::RegExp(a), Value::RegExp(b)) => Rc::ptr_eq(a, b),
        (Value::Date(a), Value::Date(b)) => Rc::ptr_eq(a, b),
        (Value::BigInt(a), Value::BigInt(b)) => Rc::ptr_eq(a, b),
        (Value::Settler(a, left_kind), Value::Settler(b, right_kind)) => {
            *left_kind == *right_kind && Rc::ptr_eq(a, b)
        }
        (Value::TypedArray(a), Value::TypedArray(b)) => Rc::ptr_eq(a, b),
        (Value::ArrayBuffer(a), Value::ArrayBuffer(b)) => Rc::ptr_eq(a, b),
        (Value::Proxy(a), Value::Proxy(b)) => Rc::ptr_eq(a, b),
        (Value::Bound(a), Value::Bound(b)) => Rc::ptr_eq(a, b),
        (Value::Native(a), Value::Native(b)) => a == b,
        (Value::Callback(a), Value::Callback(b)) => a == b,
        _ => false,
    }
}

impl<H: HostOps + ?Sized> Vm<'_, H> {
    fn new<'host>(
        program: Rc<Program>,
        locals: Vec<Value>,
        host: &'host mut H,
        limits: ExecutionLimits,
        realm: &Realm,
    ) -> Vm<'host, H> {
        realm.remember_program(Rc::clone(&program));
        Vm {
            program,
            host,
            stack: Vec::new(),
            locals,
            function: None,
            suspended: false,
            handlers: Vec::new(),
            arguments: Vec::new(),
            this_value: Value::Undefined,
            realm: Rc::clone(&realm.state),
            native_depth: 0,
            suspended_awaiting: false,
            new_target: Value::Undefined,
            frames: Vec::new(),
            instruction: 0,
            caught: None,
            steps: 0,
            limits,
        }
    }

    fn private_property(&self, target: &Value, key: &PropertyKey) -> Option<Property> {
        if !private_target(target) {
            return None;
        }
        self.realm
            .private_slots
            .borrow()
            .iter()
            .find(|(owner, _)| same_private_target(owner, target))
            .and_then(|(_, data)| data.get_own_property(key.clone()).cloned())
    }

    fn private_has(&self, target: &Value, key: &PropertyKey) -> bool {
        self.private_property(target, key).is_some()
    }

    fn init_private(&self, target: &Value, key: PropertyKey, value: Value) -> bool {
        if !private_target(target) {
            return false;
        }
        let mut slots = self.realm.private_slots.borrow_mut();
        if let Some((_, data)) = slots
            .iter_mut()
            .find(|(owner, _)| same_private_target(owner, target))
        {
            data.set_private(key, value);
        } else {
            let mut data = ObjectData::default();
            data.set_private(key, value);
            slots.push((target.clone(), data));
        }
        true
    }

    fn define_private_accessor(
        &self,
        target: &Value,
        key: PropertyKey,
        getter: Option<Value>,
        setter: Option<Value>,
    ) -> bool {
        if !private_target(target) {
            return false;
        }
        let mut slots = self.realm.private_slots.borrow_mut();
        if let Some((_, data)) = slots
            .iter_mut()
            .find(|(owner, _)| same_private_target(owner, target))
        {
            data.define_accessor(key, getter, setter);
        } else {
            let mut data = ObjectData::default();
            data.define_accessor(key, getter, setter);
            slots.push((target.clone(), data));
        }
        true
    }

    fn run(&mut self) -> Result<ExecutionResult, ExecutionError> {
        // `usize::MAX` never matches a real frame depth, so the loop ends only at the outermost
        // `Return` -- the end of the program.
        self.run_frames(usize::MAX)
    }

    /// Run until the frame stack drops back to `stop_at`, or until the program ends.
    ///
    /// The boundary is what lets a built-in call back into user code: `map` pushes a frame for
    /// its callback and then drives the machine until that frame returns, rather than needing a
    /// second interpreter or a continuation.
    fn run_frames(&mut self, stop_at: usize) -> Result<ExecutionResult, ExecutionError> {
        // Held across steps and refreshed only when the running frame's program actually
        // changes. The borrow has to come from a local rather than from `self`, which every
        // instruction below needs mutably -- but paying a refcount pair per *instruction* to
        // arrange that is a real cost, and a pointer comparison is not.
        let mut program = Rc::clone(&self.program);
        loop {
            if self.steps >= self.limits.max_steps {
                return Err(ExecutionError::StepLimitExceeded {
                    steps: self.steps,
                    limit: self.limits.max_steps,
                });
            }
            if !Rc::ptr_eq(&program, &self.program) {
                program = Rc::clone(&self.program);
            }
            let Some(opcode) = Self::code(&program, self.function.as_ref()).get(self.instruction)
            else {
                return Err(ExecutionError::InvalidInstructionPointer {
                    instruction: self.instruction,
                });
            };
            let current = self.instruction;
            self.steps += 1;
            self.instruction += 1;

            match opcode {
                Instruction::Const(index) => {
                    let Some(value) = program.constants.get(*index as usize) else {
                        return Err(ExecutionError::ConstantOutOfBounds {
                            index: *index as usize,
                        });
                    };
                    self.stack.push(value.clone().into());
                }
                Instruction::LoadWellKnown(symbol) => {
                    self.stack
                        .push(Value::Symbol(Symbol::WellKnown(match symbol {
                            WellKnownSymbol::Iterator => WellKnown::Iterator,
                            WellKnownSymbol::AsyncIterator => WellKnown::AsyncIterator,
                        })));
                }
                Instruction::LoadLocal(index) => {
                    let Some(value) = self.locals.get(*index as usize) else {
                        return Err(ExecutionError::LocalOutOfBounds {
                            index: *index as usize,
                        });
                    };
                    self.stack.push(value.clone());
                }
                Instruction::StoreLocal(index) => {
                    let value = self.pop(current)?;
                    let Some(local) = self.locals.get_mut(*index as usize) else {
                        return Err(ExecutionError::LocalOutOfBounds {
                            index: *index as usize,
                        });
                    };
                    *local = value;
                }
                Instruction::Pop => {
                    self.pop(current)?;
                }
                Instruction::Dup => {
                    let value =
                        self.stack
                            .last()
                            .cloned()
                            .ok_or(ExecutionError::StackUnderflow {
                                instruction: current,
                            })?;
                    self.stack.push(value);
                }
                Instruction::Unary(operator) => {
                    let value = self.pop(current)?;
                    // Only the numeric operators coerce. `typeof` reports a type, `void`
                    // discards, and `!` asks whether the value is truthy -- an object always is,
                    // whatever its `valueOf` would have said, so reducing it would both be wrong
                    // and run the program's code for no reason.
                    let value = if matches!(
                        operator,
                        UnaryOp::Negate | UnaryOp::Plus | UnaryOp::BitwiseNot
                    ) {
                        self.as_primitive(&value, false, current)?
                    } else {
                        value
                    };
                    self.stack.push(unary(*operator, value));
                }
                // `in` consults a proxy's `has` trap, so it cannot go through the pure operator
                // table the way the arithmetic ones do.
                Instruction::Binary(BinaryOp::In)
                    if matches!(self.stack.last(), Some(Value::Proxy(_))) =>
                {
                    let right = self.pop(current)?;
                    let left = self.pop(current)?;
                    let found = self.has(&right, &left, current);
                    let Some(found) = self.catchable(found, current)? else {
                        continue;
                    };
                    self.stack.push(Value::Boolean(found));
                }
                Instruction::Binary(BinaryOp::InstanceOf) => {
                    let right = self.pop(current)?;
                    let left = self.pop(current)?;
                    // Threaded rather than global: the error prototypes belong to this run, and
                    // creating the one being tested against is what makes the comparison
                    // meaningful for a kind nothing has constructed yet.
                    let prototype = match &right {
                        Value::Native(Native::Error(kind)) => Some(self.error_prototype(*kind)),
                        _ => None,
                    };
                    self.stack.push(Value::Boolean(instance_of(
                        &left,
                        &right,
                        prototype.as_ref(),
                    )));
                }
                Instruction::Binary(operator) => {
                    let right = self.pop(current)?;
                    let left = self.pop(current)?;
                    // Every operator that coerces its operands reduces an object to a primitive
                    // first, which is a *call* -- so it cannot happen inside `binary`, which has
                    // no machine to call with. That is all of the arithmetic and the ordering
                    // comparisons: `a - b` where both are wrappers around a number is how every
                    // date and decimal library is used, and it was reaching `toString` and
                    // producing NaN. The equality operators are excluded because they compare by
                    // identity when both sides are objects, and `in`/`instanceof` because their
                    // right side is a container rather than a value.
                    // Every operator that coerces its operands reduces an object to a primitive
                    // first, which is a *call* -- so it cannot happen inside `binary`, which has
                    // no machine to call with. `a - b` where both sides wrap a number is how
                    // every date and decimal library is used, and it was reaching `toString` and
                    // producing NaN.
                    //
                    // `==` is the awkward one: it compares two objects by identity and reduces
                    // only when the other side is a primitive. `===` never reduces, and
                    // `in`/`instanceof` take a container on the right rather than a value.
                    // `x == null` compares against nullish, which the specification answers
                    // without any conversion -- so reducing there would run the program's
                    // `valueOf` for a question that never needed it. `value != null` is the
                    // commonest guard in JavaScript, and calling into a wrapper's accessor to
                    // evaluate it is both wrong and, for a wrapper that assumes it is being used
                    // properly, fatal.
                    let mixed = is_object_like(&left) != is_object_like(&right)
                        && !left.is_nullish()
                        && !right.is_nullish();
                    let reduces =
                        matches!(
                            operator,
                            BinaryOp::Add
                                | BinaryOp::Subtract
                                | BinaryOp::Multiply
                                | BinaryOp::Divide
                                | BinaryOp::Remainder
                                | BinaryOp::Exponentiate
                                | BinaryOp::LessThan
                                | BinaryOp::LessThanOrEqual
                                | BinaryOp::GreaterThan
                                | BinaryOp::GreaterThanOrEqual
                                | BinaryOp::BitwiseAnd
                                | BinaryOp::BitwiseOr
                                | BinaryOp::BitwiseXor
                                | BinaryOp::ShiftLeft
                                | BinaryOp::ShiftRight
                                | BinaryOp::UnsignedShiftRight
                        ) || (matches!(operator, BinaryOp::LooseEqual | BinaryOp::LooseNotEqual)
                            && mixed);
                    let (left, right) = if reduces {
                        (
                            self.as_primitive(&left, false, current)?,
                            self.as_primitive(&right, false, current)?,
                        )
                    } else {
                        (left, right)
                    };
                    if let Some(outcome) = bigint_binary(*operator, &left, &right) {
                        let Some(value) = self.catchable(outcome, current)? else {
                            continue;
                        };
                        self.stack.push(value);
                        continue;
                    }
                    self.stack.push(binary(*operator, left, right));
                }
                Instruction::JumpIfFalse(target) => {
                    if !self.pop(current)?.is_truthy() {
                        self.jump(current, *target)?;
                    }
                }
                Instruction::JumpIfTruthy(target) => {
                    if self.pop(current)?.is_truthy() {
                        self.jump(current, *target)?;
                    }
                }
                Instruction::JumpIfNotNullish(target) => {
                    if !self.pop(current)?.is_nullish() {
                        self.jump(current, *target)?;
                    }
                }
                Instruction::Jump(target) => self.jump(current, *target)?,
                Instruction::HostCall { op, argc } => {
                    let argc = usize::from(*argc);
                    if self.stack.len() < argc {
                        return Err(ExecutionError::StackUnderflow {
                            instruction: current,
                        });
                    }
                    let args_start = self.stack.len() - argc;
                    let args = self
                        .stack
                        .drain(args_start..)
                        .map(HostValue::from)
                        .collect::<Vec<_>>();
                    let result =
                        self.host
                            .call(*op, &args)
                            .map_err(|error| ExecutionError::Host {
                                instruction: current,
                                message: error.to_string(),
                            })?;
                    self.stack.push(result.into());
                }
                Instruction::MakeFunction(index) => {
                    let closure = self.make_closure(*index)?;
                    self.stack.push(Value::Function(closure));
                }
                Instruction::CallMethod { name, argc } => {
                    let name = self.string_constant(*name, current)?;
                    let arguments = self.take_call_arguments(*argc, current)?;
                    let receiver = self.pop(current)?;
                    let read = self.read_property(&receiver, &name, current);
                    let Some(method) = self.catchable(read, current)? else {
                        continue;
                    };
                    // Named, because "undefined is not a function" is the least useful message a
                    // runtime produces and the name is right here.
                    if !is_callable(&method) {
                        self.throw_error(
                            ErrorKind::Type,
                            format!("{}.{name} is not a function", receiver.type_name()),
                            current,
                        )?;
                        continue;
                    }
                    let invoked = self.invoke(method, receiver, arguments, current);
                    let Some(result) = self.catchable(invoked, current)? else {
                        continue;
                    };
                    if let Some(result) = result {
                        self.stack.push(result);
                    }
                }
                Instruction::CallComputed { argc } => {
                    let arguments = self.take_call_arguments(*argc, current)?;
                    let key = self.pop(current)?;
                    let receiver = self.pop(current)?;
                    let method = self.read_computed(&receiver, &key, current);
                    let Some(method) = self.catchable(method, current)? else {
                        continue;
                    };
                    let invoked = self.invoke(method, receiver, arguments, current);
                    let Some(result) = self.catchable(invoked, current)? else {
                        continue;
                    };
                    if let Some(result) = result {
                        self.stack.push(result);
                    }
                }
                Instruction::Dup2 => {
                    let len = self.stack.len();
                    let (first, second) = (
                        self.stack
                            .get(len.wrapping_sub(2))
                            .ok_or(ExecutionError::StackUnderflow {
                                instruction: current,
                            })?
                            .clone(),
                        self.stack
                            .get(len.wrapping_sub(1))
                            .ok_or(ExecutionError::StackUnderflow {
                                instruction: current,
                            })?
                            .clone(),
                    );
                    self.stack.push(first);
                    self.stack.push(second);
                }
                Instruction::NewArray { count } => {
                    let count = *count as usize;
                    if self.stack.len() < count {
                        return Err(ExecutionError::StackUnderflow {
                            instruction: current,
                        });
                    }
                    let items = self.stack.split_off(self.stack.len() - count);
                    self.stack.push(Value::Array(ArrayData::new(items)));
                }
                Instruction::IterableToArray => {
                    let source = self.pop(current)?;
                    // An array is already the right shape; anything else goes through the
                    // protocol, so `const [a, b] = someSet` binds what iterating it would yield.
                    let items = match &source {
                        Value::Array(_) => {
                            let method = self.read_property(&source, iterator_key(), current)?;
                            if matches!(method, Value::Native(Native::ArrayValues)) {
                                if let Value::Array(items) = &source {
                                    dense_items(&items.borrow())
                                } else {
                                    unreachable!()
                                }
                            } else {
                                self.drain_iterable_with_method(&source, method, current)?
                            }
                        }
                        other => self.drain_iterable(other, current)?,
                    };
                    self.stack.push(Value::Array(ArrayData::new(items)));
                }
                Instruction::SpreadInto => {
                    let source = self.pop(current)?;
                    let target = self.pop(current)?;
                    let Value::Array(items) = &target else {
                        return Err(ExecutionError::NotAnObject {
                            instruction: current,
                            value: target.type_name(),
                        });
                    };
                    // Spread consumes any iterable, not only an array, so it goes through the
                    // protocol for anything else -- the same rule `for-of` follows.
                    let collected = match &source {
                        Value::Array(source) => {
                            let array = Value::Array(Rc::clone(source));
                            let method = self.read_property(&array, iterator_key(), current)?;
                            if matches!(method, Value::Native(Native::ArrayValues)) {
                                dense_items(&source.borrow())
                            } else {
                                self.drain_iterable_with_method(&array, method, current)?
                            }
                        }
                        other => self.drain_iterable(other, current)?,
                    };
                    items.borrow_mut().extend(collected);
                    self.stack.push(target);
                }
                Instruction::GetComputed => {
                    let key = self.pop(current)?;
                    let target = self.pop(current)?;
                    if target.is_nullish() {
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "Cannot read properties of {} (reading '{}')",
                                target.to_display_string(),
                                key.to_display_string()
                            ),
                            current,
                        )?;
                        continue;
                    }
                    let read = self.read_computed(&target, &key, current);
                    let Some(value) = self.catchable(read, current)? else {
                        continue;
                    };
                    self.stack.push(value);
                }
                Instruction::SetComputed => {
                    let value = self.pop(current)?;
                    let key = self.pop(current)?;
                    let target = self.pop(current)?;
                    let assigned = self.set_computed(&target, &key, value.clone(), current);
                    if self.catchable(assigned, current)?.is_none() {
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::SetPrototype => {
                    let prototype = self.pop(current)?;
                    let target = self.pop(current)?;
                    match &target {
                        Value::Object(object) => object.borrow_mut().set_prototype(prototype),
                        Value::Function(closure) => {
                            // A derived constructor inherits its parent's statics through the
                            // constructor's own property store.
                            closure.properties.borrow_mut().set_prototype(prototype);
                        }
                        other => {
                            return Err(ExecutionError::NotAnObject {
                                instruction: current,
                                value: other.type_name(),
                            });
                        }
                    }
                    self.stack.push(target);
                }
                Instruction::SuperForward => {
                    let parent = self.pop(current)?;
                    let arguments = self.arguments.clone();
                    self.stack.push(parent);
                    let argc = arguments.len();
                    for argument in arguments {
                        self.stack.push(argument);
                    }
                    // Fall through to the shared super-call path by re-dispatching with a known
                    // count: forwarding differs from `super(a, b)` only in where the count came
                    // from.
                    self.super_call(argc as u16, current)?;
                }
                Instruction::SuperCall { argc } => {
                    self.super_call(*argc, current)?;
                }
                Instruction::PushHandler(target) => {
                    self.handlers.push(Handler {
                        target: *target as usize,
                        frame_depth: self.frames.len(),
                        stack_height: self.stack.len(),
                        native_depth: self.native_depth,
                    });
                }
                Instruction::PopHandler => {
                    self.handlers.pop();
                }
                // Identical in the machine: both leave the generator's registers in place for
                // the caller to snapshot, where a `Return` would have popped the frame. They
                // differ only in what they tell the driver, which is recorded on the state by
                // whoever resumed it.
                Instruction::Yield | Instruction::Await => {
                    let value = self.pop(current)?;
                    self.suspended = true;
                    self.suspended_awaiting = matches!(opcode, Instruction::Await);
                    return Ok(ExecutionResult {
                        value,
                        steps: self.steps,
                    });
                }
                Instruction::Throw => {
                    let thrown = self.pop(current)?;
                    self.unwind(thrown, current)?;
                }
                Instruction::LoadArguments => {
                    let arguments = self.visible_arguments();
                    self.stack.push(Value::Array(ArrayData::new(arguments)));
                }
                // A rest parameter is always the *call's* arguments, arrow or not.
                Instruction::RestArguments(from) => {
                    let rest = self
                        .arguments
                        .get(*from as usize..)
                        .unwrap_or_default()
                        .to_vec();
                    self.stack.push(Value::Array(ArrayData::new(rest)));
                }
                Instruction::LoadThis => {
                    let this_value = self.this_value.clone();
                    self.stack.push(this_value);
                }
                Instruction::LoadNewTarget => {
                    let new_target = self.new_target.clone();
                    self.stack.push(new_target);
                }
                Instruction::GetProperty(name)
                    if matches!(self.stack.last(), Some(Value::Native(Native::Error(_))))
                        && self.string_constant(*name, current)? == "prototype" =>
                {
                    // `Error.prototype` has to be the same object instances inherit from, or
                    // `extends Error` would link to something no error descends from.
                    let Some(Value::Native(Native::Error(kind))) = self.stack.pop() else {
                        unreachable!("checked by the guard");
                    };
                    let prototype = self.error_prototype(kind);
                    self.stack.push(prototype);
                }
                Instruction::LoadBuiltin(builtin) => {
                    self.stack.push(Value::Native(match builtin {
                        kiln_ir::Builtin::Error => Native::Error(ErrorKind::Error),
                        kiln_ir::Builtin::TypeError => Native::Error(ErrorKind::Type),
                        kiln_ir::Builtin::RangeError => Native::Error(ErrorKind::Range),
                        kiln_ir::Builtin::ReferenceError => Native::Error(ErrorKind::Reference),
                        kiln_ir::Builtin::SyntaxError => Native::Error(ErrorKind::Syntax),
                        kiln_ir::Builtin::Symbol => Native::SymbolConstructor,
                        kiln_ir::Builtin::String => Native::StringConstructor,
                        kiln_ir::Builtin::Set => Native::SetConstructor,
                        kiln_ir::Builtin::Map => Native::MapConstructor,
                        kiln_ir::Builtin::GlobalThis => {
                            let globals = self.global_object();
                            self.stack.push(globals);
                            continue;
                        }
                        kiln_ir::Builtin::Process => {
                            let process = self.process_object();
                            self.stack.push(process);
                            continue;
                        }
                        kiln_ir::Builtin::Array => Native::ArrayConstructor,
                        kiln_ir::Builtin::Object => Native::ObjectConstructor,
                        kiln_ir::Builtin::Json => Native::Json,
                        kiln_ir::Builtin::Math => Native::Math,
                        kiln_ir::Builtin::Number => Native::NumberConstructor,
                        kiln_ir::Builtin::Boolean => Native::BooleanConstructor,
                        kiln_ir::Builtin::ParseInt => Native::ParseInt,
                        kiln_ir::Builtin::Escape => Native::Escape,
                        kiln_ir::Builtin::Unescape => Native::Unescape,
                        kiln_ir::Builtin::EncodeUri => Native::EncodeUri,
                        kiln_ir::Builtin::EncodeUriComponent => Native::EncodeUriComponent,
                        kiln_ir::Builtin::DecodeUri => Native::DecodeUri,
                        kiln_ir::Builtin::DecodeUriComponent => Native::DecodeUriComponent,
                        kiln_ir::Builtin::ParseFloat => Native::ParseFloat,
                        kiln_ir::Builtin::IsNaN => Native::IsNaN,
                        kiln_ir::Builtin::IsFinite => Native::IsFinite,
                        kiln_ir::Builtin::SetTimeout => Native::SetTimeout,
                        kiln_ir::Builtin::SetInterval => Native::SetInterval,
                        kiln_ir::Builtin::ClearTimeout => Native::ClearTimeout,
                        kiln_ir::Builtin::ClearInterval => Native::ClearInterval,
                        kiln_ir::Builtin::QueueMicrotask => Native::QueueMicrotask,
                        kiln_ir::Builtin::Promise => Native::PromiseConstructor,
                        kiln_ir::Builtin::RegExp => Native::RegExpConstructor,
                        kiln_ir::Builtin::Date => Native::DateConstructor,
                        kiln_ir::Builtin::WeakMap => Native::WeakMapConstructor,
                        kiln_ir::Builtin::WeakSet => Native::WeakSetConstructor,
                        kiln_ir::Builtin::Performance => Native::Performance,
                        kiln_ir::Builtin::Reflect => Native::Reflect,
                        kiln_ir::Builtin::BigInt => Native::BigIntConstructor,
                        kiln_ir::Builtin::Function => Native::FunctionConstructor,
                        kiln_ir::Builtin::Proxy => Native::ProxyConstructor,
                        kiln_ir::Builtin::ArrayBuffer => Native::ArrayBufferConstructor,
                        kiln_ir::Builtin::Int8Array => {
                            Native::TypedArrayConstructor(TypedKind::Int8)
                        }
                        kiln_ir::Builtin::Uint8Array => {
                            Native::TypedArrayConstructor(TypedKind::Uint8)
                        }
                        kiln_ir::Builtin::Uint8ClampedArray => {
                            Native::TypedArrayConstructor(TypedKind::Uint8Clamped)
                        }
                        kiln_ir::Builtin::Int16Array => {
                            Native::TypedArrayConstructor(TypedKind::Int16)
                        }
                        kiln_ir::Builtin::Uint16Array => {
                            Native::TypedArrayConstructor(TypedKind::Uint16)
                        }
                        kiln_ir::Builtin::Int32Array => {
                            Native::TypedArrayConstructor(TypedKind::Int32)
                        }
                        kiln_ir::Builtin::Uint32Array => {
                            Native::TypedArrayConstructor(TypedKind::Uint32)
                        }
                        kiln_ir::Builtin::Float32Array => {
                            Native::TypedArrayConstructor(TypedKind::Float32)
                        }
                        kiln_ir::Builtin::Float64Array => {
                            Native::TypedArrayConstructor(TypedKind::Float64)
                        }

                        // Values rather than callables, so they leave the loop early like
                        // `globalThis` does rather than being wrapped in a `Native`.
                        kiln_ir::Builtin::NotANumber => {
                            self.stack.push(Value::Number(f64::NAN));
                            continue;
                        }
                        kiln_ir::Builtin::Infinity => {
                            self.stack.push(Value::Number(f64::INFINITY));
                            continue;
                        }
                    }));
                }
                Instruction::New { argc } => {
                    let argc = *argc as usize;
                    if self.stack.len() < argc + 1 {
                        return Err(ExecutionError::StackUnderflow {
                            instruction: current,
                        });
                    }
                    let arguments = self.stack.split_off(self.stack.len() - argc);
                    let callee = self.pop(current)?;
                    let constructed = self.construct(callee, arguments, current);
                    if self.catchable(constructed, current)?.is_none() {
                        continue;
                    }
                }
                Instruction::NewObject { count } => {
                    let count = *count as usize;
                    if self.stack.len() < count * 2 {
                        return Err(ExecutionError::StackUnderflow {
                            instruction: current,
                        });
                    }
                    let flat = self.stack.split_off(self.stack.len() - count * 2);
                    let mut object = ObjectData::default();
                    for pair in flat.chunks_exact(2) {
                        // Any value can be a computed key: it is converted the same way a
                        // property read converts one, so `{ [Symbol.iterator]: f }` and
                        // `o[Symbol.iterator]` name the same property. A duplicate key keeps
                        // the last value and the first position, which is what `set` does.
                        object.set(pair[0].to_property_key(), pair[1].clone());
                    }
                    self.stack
                        .push(Value::Object(Rc::new(RefCell::new(object))));
                }
                Instruction::SpreadObject => {
                    let source = self.pop(current)?;
                    let target = self.pop(current)?;
                    let Value::Object(object) = &target else {
                        return Err(ExecutionError::NotAnObject {
                            instruction: current,
                            value: target.type_name(),
                        });
                    };
                    // Spreading `null` or `undefined` contributes nothing rather than throwing,
                    // which is the one place object spread is more forgiving than array spread.
                    let object = Rc::clone(object);
                    for (key, property) in self.enumerable_own_properties(&source, current)? {
                        // A spread copies *values*: it reads each property, which for an
                        // accessor means calling its getter, and the result is a plain data
                        // property on the target.
                        let value = match property {
                            Property::Value(value) => value,
                            Property::Accessor { get, .. } => {
                                // Setter-only: there is nothing to read, and a spread copies
                                // what a read produces.
                                let Some(getter) = get else {
                                    continue;
                                };
                                self.call_value(&getter, source.clone(), Vec::new(), current)?
                            }
                        };
                        object.borrow_mut().set(key, value);
                    }
                    self.stack.push(target);
                }
                Instruction::EnumerateKeys => {
                    let target = self.pop(current)?;
                    // Own enumerable *string* keys only. The VM's reserved keys are symbols and
                    // internal slots, and neither is enumerable; inherited properties are absent
                    // because nothing here can yet make one enumerable -- a class puts its
                    // methods on the prototype, and those are not.
                    let keys = if matches!(target, Value::Proxy(_)) {
                        self.own_keys(&target, current)?
                    } else {
                        own_properties(&target)
                            .into_iter()
                            .map(|(key, _)| key)
                            .collect()
                    };
                    let keys = keys
                        .into_iter()
                        .filter_map(|key| string_key(&key).map(str::to_owned).map(Value::String))
                        .collect();
                    self.stack.push(Value::Array(ArrayData::new(keys)));
                }
                Instruction::Apply => {
                    let arguments = self.pop(current)?;
                    let callee = self.pop(current)?;
                    let this_value = self.pop(current)?;
                    let Value::Array(arguments) = arguments else {
                        // The compiler only ever builds this operand with `array_literal`.
                        return Err(ExecutionError::NotAnObject {
                            instruction: current,
                            value: arguments.type_name(),
                        });
                    };
                    let arguments = arguments.borrow().clone();
                    let result = self.call_value(&callee, this_value, arguments, current)?;
                    self.stack.push(result);
                }
                Instruction::ConstructSpread => {
                    let arguments = self.pop(current)?;
                    let callee = self.pop(current)?;
                    let Value::Array(arguments) = arguments else {
                        return Err(ExecutionError::NotAnObject {
                            instruction: current,
                            value: arguments.type_name(),
                        });
                    };
                    let arguments = arguments.borrow().clone();
                    // `new` with a computed argument list is the same construction the
                    // fixed-arity `New` performs, so it goes through the same path rather than
                    // repeating the allocate-run-override sequence.
                    let constructed = self.construct(callee, arguments, current);
                    if self.catchable(constructed, current)?.is_none() {
                        continue;
                    }
                }
                Instruction::MarkUninitialized(slot) => {
                    let slot = *slot as usize;
                    let marker = Value::Native(Native::Uninitialized);
                    match self.locals.get_mut(slot) {
                        // A cell may already be in the slot, because the binding is captured and
                        // the cell has to exist before anything captures it.
                        Some(Value::Cell(cell)) => *cell.borrow_mut() = marker,
                        Some(local) => *local = marker,
                        None => return Err(ExecutionError::LocalOutOfBounds { index: slot }),
                    }
                }
                Instruction::ThrowReference(name) => {
                    let name = self.string_constant(*name, current)?;
                    self.throw_error(
                        ErrorKind::Reference,
                        format!("{name} is not defined"),
                        current,
                    )?;
                    continue;
                }
                Instruction::PushHole => self.stack.push(Value::Native(Native::Hole)),
                Instruction::NewRegExp { pattern, flags } => {
                    let pattern = self.string_constant(*pattern, current)?;
                    let flags = self.string_constant(*flags, current)?;
                    match build_regexp(&pattern, &flags) {
                        Some(regexp) => self.stack.push(regexp),
                        // A pattern oxc parsed but this engine rejects. A SyntaxError is what a
                        // program would get from `new RegExp` with the same text.
                        None => {
                            self.throw_error(
                                ErrorKind::Syntax,
                                format!("Invalid regular expression: /{pattern}/{flags}"),
                                current,
                            )?;
                            continue;
                        }
                    }
                }
                Instruction::PrivateIn(name) => {
                    let name = self.string_constant(*name, current)?;
                    let target = self.pop(current)?;
                    if !private_target(&target) {
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "right-hand side of `#{} in` must be an object, got {}",
                                private_display_name(&name),
                                target.type_name()
                            ),
                            current,
                        )?;
                        continue;
                    }
                    let found = self.private_has(&target, &internal_key(&name));
                    self.stack.push(Value::Boolean(found));
                }
                Instruction::GetPrivate(name) => {
                    let name = self.string_constant(*name, current)?;
                    let target = self.pop(current)?;
                    // Own-only, and a miss is a TypeError. That own-only rule *is* the brand
                    // check: an object of another class does not carry the name, however much
                    // of the rest of the shape it shares.
                    let Some(property) = self.private_property(&target, &internal_key(&name))
                    else {
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "Cannot read private member #{} from an object whose class did not declare it",
                                private_display_name(&name)
                            ),
                            current,
                        )?;
                        continue;
                    };
                    match property {
                        Property::Value(value) => self.stack.push(value),
                        Property::Accessor {
                            get: Some(getter), ..
                        } => {
                            let read = self.call_value(&getter, target, Vec::new(), current);
                            let Some(value) = self.catchable(read, current)? else {
                                continue;
                            };
                            self.stack.push(value);
                        }
                        Property::Accessor { get: None, .. } => {
                            self.throw_error(
                                ErrorKind::Type,
                                format!(
                                    "Cannot read private member #{} which has no getter",
                                    private_display_name(&name)
                                ),
                                current,
                            )?;
                            continue;
                        }
                    }
                }
                Instruction::SetPrivate(name) => {
                    let name = self.string_constant(*name, current)?;
                    let value = self.pop(current)?;
                    let target = self.pop(current)?;
                    let private_key = internal_key(&name);
                    let Some(property) = self.private_property(&target, &private_key) else {
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "Cannot write private member #{} to an object whose class did not declare it",
                                private_display_name(&name)
                            ),
                            current,
                        )?;
                        continue;
                    };
                    match property {
                        Property::Value(_) => {
                            // Private writes bypass ObjectData's frozen/extensible checks.
                            self.init_private(&target, private_key, value.clone());
                        }
                        Property::Accessor {
                            set: Some(setter), ..
                        } => {
                            let written =
                                self.call_value(&setter, target, vec![value.clone()], current);
                            if self.catchable(written, current)?.is_none() {
                                continue;
                            }
                        }
                        Property::Accessor { set: None, .. } => {
                            self.throw_error(
                                ErrorKind::Type,
                                format!(
                                    "Cannot write private member #{} which has no setter",
                                    private_display_name(&name)
                                ),
                                current,
                            )?;
                            continue;
                        }
                    }
                    self.stack.push(value);
                }
                Instruction::InitPrivate(name) => {
                    let name = self.string_constant(*name, current)?;
                    let value = self.pop(current)?;
                    let target = self.pop(current)?;
                    if !self.init_private(&target, internal_key(&name), value.clone()) {
                        return Err(ExecutionError::NotAnObject {
                            instruction: current,
                            value: target.type_name(),
                        });
                    }
                    self.stack.push(value);
                }
                Instruction::DefinePrivateAccessor { name, setter } => {
                    let name = self.string_constant(*name, current)?;
                    let function = self.pop(current)?;
                    let target = self.pop(current)?;
                    let (getter, setter) = if *setter {
                        (None, Some(function))
                    } else {
                        (Some(function), None)
                    };
                    if !self.define_private_accessor(&target, internal_key(&name), getter, setter) {
                        return Err(ExecutionError::NotAnObject {
                            instruction: current,
                            value: target.type_name(),
                        });
                    }
                    self.stack.push(target);
                }
                Instruction::DefineAccessor { setter } => {
                    let function = self.pop(current)?;
                    let key = self.pop(current)?;
                    let target = self.pop(current)?;
                    let name = key.to_property_key();
                    let (getter, setter) = if *setter {
                        (None, Some(function))
                    } else {
                        (Some(function), None)
                    };
                    match &target {
                        Value::Object(object) => {
                            object.borrow_mut().define_accessor(&name, getter, setter);
                        }
                        // A static accessor lands on the constructor, whose properties live in
                        // its own store rather than in an object.
                        Value::Function(closure) => {
                            closure
                                .properties
                                .borrow_mut()
                                .define_accessor(&name, getter, setter);
                        }
                        other => {
                            return Err(ExecutionError::NotAnObject {
                                instruction: current,
                                value: other.type_name(),
                            });
                        }
                    }
                    self.stack.push(target);
                }
                Instruction::DeleteComputed => {
                    let key = self.pop(current)?;
                    let target = self.pop(current)?;
                    let removed = self.delete(&target, &key, current);
                    let Some(removed) = self.catchable(removed, current)? else {
                        continue;
                    };
                    self.stack.push(Value::Boolean(removed));
                }
                Instruction::DeleteProperty(name) => {
                    let name = self.string_constant(*name, current)?;
                    let target = self.pop(current)?;
                    let removed = self.delete(&target, &Value::String(name), current);
                    let Some(removed) = self.catchable(removed, current)? else {
                        continue;
                    };
                    self.stack.push(Value::Boolean(removed));
                }
                Instruction::GetProperty(name) => {
                    let name = self.string_constant(*name, current)?;
                    let target = self.pop(current)?;
                    if target.is_nullish() {
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "Cannot read properties of {} (reading '{name}')",
                                target.to_display_string()
                            ),
                            current,
                        )?;
                        continue;
                    }
                    // Catchable: a proxy trap and a getter are both ordinary code, and what
                    // they throw is the program's to handle.
                    let read = self.read_property(&target, &name, current);
                    let Some(value) = self.catchable(read, current)? else {
                        continue;
                    };
                    self.stack.push(value);
                }
                Instruction::SetProperty(name) => {
                    let name = self.string_constant(*name, current)?;
                    let value = self.pop(current)?;
                    let target = self.pop(current)?;
                    // Assigning a property of a primitive is silently discarded in sloppy mode
                    // and a TypeError in strict mode. Modules are always strict, and there is
                    // nowhere to throw yet, so it is an execution error.
                    let assigned =
                        self.set_computed(&target, &Value::String(name), value.clone(), current);
                    if self.catchable(assigned, current)?.is_none() {
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::NewCell(slot) => {
                    let slot = *slot as usize;
                    let cell = Rc::new(RefCell::new(Value::Undefined));
                    *self
                        .locals
                        .get_mut(slot)
                        .ok_or(ExecutionError::LocalOutOfBounds { index: slot })? =
                        Value::Cell(cell);
                }
                Instruction::LoadCell(slot) => {
                    let cell = self.cell(*slot as usize)?;
                    let value = cell.borrow().clone();
                    if matches!(value, Value::Native(Native::Uninitialized)) {
                        self.throw_error(
                            ErrorKind::Reference,
                            "cannot access a binding before its declaration".to_owned(),
                            current,
                        )?;
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::StoreCell(slot) => {
                    let value = self.pop(current)?;
                    let cell = self.cell(*slot as usize)?;
                    *cell.borrow_mut() = value;
                }
                Instruction::LoadUpvalue(index) => {
                    let value = self.upvalue(*index)?.borrow().clone();
                    if matches!(value, Value::Native(Native::Uninitialized)) {
                        self.throw_error(
                            ErrorKind::Reference,
                            "cannot access a binding before its declaration".to_owned(),
                            current,
                        )?;
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::StoreUpvalue(index) => {
                    let value = self.pop(current)?;
                    *self.upvalue(*index)?.borrow_mut() = value;
                }
                Instruction::Call { argc } => {
                    let argc = *argc;
                    let depth = argc as usize;
                    let callee = self
                        .stack
                        .len()
                        .checked_sub(depth + 1)
                        .and_then(|index| self.stack.get(index))
                        .ok_or(ExecutionError::StackUnderflow {
                            instruction: current,
                        })?;
                    match callee {
                        Value::Function(closure) => {
                            let closure = Rc::clone(closure);
                            // Through `catchable`, so running out of frames is a `RangeError`
                            // the program can catch rather than the end of the run. A parser
                            // that reports a nesting limit instead of dying is written exactly
                            // this way, and so is every "is this too deep" guard.
                            let called = self.call(closure, argc, current);
                            if self.catchable(called, current)?.is_none() {
                                continue;
                            }
                        }
                        // A promise executor's `resolve`/`reject`: an ordinary callable value,
                        // which reaches the program as one and so has to be callable here.
                        Value::Settler(..) => {
                            let callee = callee.clone();
                            let arguments = self.take_arguments(argc, current)?;
                            let called =
                                self.call_value(&callee, Value::Undefined, arguments, current);
                            let Some(result) = self.catchable(called, current)? else {
                                continue;
                            };
                            self.stack.push(result);
                        }
                        // Both defer to `call_value`: a bound function knows its own receiver,
                        // and a proxy has to consult its `apply` trap before anything else can
                        // happen.
                        Value::Bound(_) | Value::Proxy(_) => {
                            let callee = callee.clone();
                            let arguments = self.take_arguments(argc, current)?;
                            let called =
                                self.call_value(&callee, Value::Undefined, arguments, current);
                            let Some(result) = self.catchable(called, current)? else {
                                continue;
                            };
                            self.stack.push(result);
                        }
                        Value::Native(native) => {
                            let native = *native;
                            let arguments = self.take_arguments(argc, current)?;
                            let called = self.dispatch_native(
                                native,
                                &Value::Undefined,
                                &arguments,
                                current,
                            );
                            let Some(result) = self.catchable(called, current)? else {
                                continue;
                            };
                            self.stack.push(result);
                        }
                        other => {
                            let value = other.type_name();
                            self.throw_error(
                                ErrorKind::Type,
                                format!("a {value} is not a function"),
                                current,
                            )?;
                            continue;
                        }
                    }
                }
                Instruction::LoadGlobal(slot) => {
                    let value = self.global(*slot as usize)?;
                    // A module binding read from inside a function that ran too early: the one
                    // dead-zone read the compiler cannot see, because whether the function is
                    // called before the declaration is a question about the run, not the text.
                    if matches!(value, Value::Native(Native::Uninitialized)) {
                        self.throw_error(
                            ErrorKind::Reference,
                            "cannot access a binding before its declaration".to_owned(),
                            current,
                        )?;
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::StoreGlobal(slot) => {
                    let value = self.pop(current)?;
                    self.set_global(*slot as usize, value)?;
                }
                Instruction::Return => {
                    let mut value = self.stack.pop().unwrap_or(Value::Undefined);
                    let Some(frame) = self.frames.pop() else {
                        return Ok(ExecutionResult {
                            value,
                            steps: self.steps,
                        });
                    };
                    // Discard anything the callee left behind. A well-formed program leaves
                    // nothing, but truncating means a malformed one cannot corrupt its caller.
                    self.stack.truncate(frame.stack_base);
                    // A handler belongs to the frame that installed it. `return` out of a `try`
                    // leaves one behind, and a later unrelated throw would then land on a target
                    // in a function that has already returned -- executing whatever instruction
                    // happens to live at that offset in the *current* function.
                    while self
                        .handlers
                        .last()
                        .is_some_and(|handler| handler.frame_depth > self.frames.len())
                    {
                        self.handlers.pop();
                    }
                    self.locals = frame.locals;
                    self.program = frame.program;
                    self.function = frame.function;
                    self.this_value = frame.this_value;
                    self.new_target = frame.new_target;
                    self.arguments = frame.arguments;
                    self.instruction = frame.instruction;
                    if let Some(instance) = frame.constructing
                        && !matches!(value, Value::Object(_) | Value::Array(_))
                    {
                        // A constructor yields its instance unless the body returned an object,
                        // which wins. That override is what makes a factory constructor work,
                        // and it is why an accidental `return 1` is harmless rather than
                        // producing a number where an instance was expected.
                        value = instance;
                    }
                    self.stack.push(value);
                    if self.frames.len() == stop_at {
                        return Ok(ExecutionResult {
                            value: self.stack.pop().unwrap_or(Value::Undefined),
                            steps: self.steps,
                        });
                    }
                }
            }
        }
    }

    /// Transfer control to the innermost handler, or fail if there is none.
    ///
    /// Unwinding restores frames one at a time rather than jumping directly, so each caller's
    /// locals, `this` and arguments are put back exactly as `Return` would have -- an exception
    /// crossing a call must leave the machine in the same shape a normal return does.
    fn unwind(&mut self, thrown: Value, instruction: usize) -> Result<(), ExecutionError> {
        // A handler installed below the current Rust re-entry cannot be jumped to from here:
        // the built-in driving this callback is still on the Rust stack, and `run_frames` would
        // carry on executing whatever the jump landed on -- which is the *caller's* code, at the
        // caller's instruction pointer, inside a driver that thinks it is still running a
        // callback. The value travels as an error instead, and the same handler takes it once
        // every Rust frame between here and the instruction loop has returned.
        //
        // A *host* handler is the exception, and the reason the check is on the target rather
        // than on the depth alone: it means the Rust frame immediately outside asked for the
        // value, so returning to it is not crossing a boundary but arriving at one.
        if self.handlers.last().is_some_and(|handler| {
            handler.target != HOST_HANDLER && handler.native_depth < self.native_depth
        }) {
            self.caught = Some(thrown);
            return Err(ExecutionError::ThrownAcrossNative);
        }
        let Some(handler) = self.handlers.pop() else {
            // A `.return()` signal that reaches the top has run every `finally` between the
            // suspension point and here, which is the whole reason it was thrown.
            if matches!(thrown, Value::Native(Native::ReturnSignal)) {
                return Err(ExecutionError::GeneratorReturned);
            }
            let trace = captured_trace(&thrown);
            return Err(ExecutionError::Uncaught {
                instruction,
                message: self.describe_uncaught(&thrown, instruction),
                trace,
            });
        };
        if handler.target == HOST_HANDLER {
            // The host asked for the value rather than a jump. Frames and stack are still
            // restored, so the machine is left exactly as an ordinary catch would leave it.
            while self.frames.len() > handler.frame_depth {
                let frame = self.frames.pop().expect("checked by the loop condition");
                self.locals = frame.locals;
                self.program = frame.program;
                self.function = frame.function;
                self.this_value = frame.this_value;
                self.new_target = frame.new_target;
                self.arguments = frame.arguments;
                self.instruction = frame.instruction;
            }
            self.stack.truncate(handler.stack_height);
            self.caught = Some(thrown);
            return Err(ExecutionError::CaughtByHost);
        }
        while self.frames.len() > handler.frame_depth {
            let frame = self.frames.pop().expect("checked by the loop condition");
            self.locals = frame.locals;
            self.program = frame.program;
            self.function = frame.function;
            self.this_value = frame.this_value;
            self.new_target = frame.new_target;
            self.arguments = frame.arguments;
            self.instruction = frame.instruction;
        }
        self.stack.truncate(handler.stack_height);
        self.stack.push(thrown);
        self.instruction = handler.target;
        Ok(())
    }

    /// Build the object a generator function call produces.
    fn build_generator(
        &mut self,
        closure: &Rc<Closure>,
        argc: u16,
        instruction: usize,
        this_value: Value,
        new_target: Value,
    ) -> Result<Value, ExecutionError> {
        let proto = &closure.program.functions[closure.function as usize];
        let argc = argc as usize;
        if self.stack.len() < argc + 1 {
            return Err(ExecutionError::StackUnderflow { instruction });
        }
        let mut arguments = self.stack.split_off(self.stack.len() - argc);
        self.stack.pop();

        let mut locals = vec![Value::Undefined; proto.local_count as usize];
        let received = arguments.clone();
        arguments.truncate(proto.parameter_count as usize);
        for (slot, argument) in arguments.into_iter().enumerate() {
            locals[slot] = argument;
        }

        Ok(Value::Generator(Rc::new(RefCell::new(GeneratorState {
            closure: Rc::clone(closure),
            locals,
            stack: Vec::new(),
            handlers: Vec::new(),
            instruction: 0,
            this_value,
            new_target,
            arguments: received,
            started: false,
            done: false,
            awaiting: false,
        }))))
    }

    /// Start or resume a generator, returning its next value and whether it finished.
    fn resume_generator(
        &mut self,
        generator: &Rc<RefCell<GeneratorState>>,
        sent: Value,
        instruction: usize,
    ) -> Result<(Value, bool), ExecutionError> {
        self.resume_generator_with(generator, Resume::Send(sent), instruction)
    }

    /// Resume a generator, optionally by throwing the `.return()` signal into it.
    fn resume_generator_with(
        &mut self,
        generator: &Rc<RefCell<GeneratorState>>,
        resume: Resume,
        instruction: usize,
    ) -> Result<(Value, bool), ExecutionError> {
        let sent = match &resume {
            Resume::Send(value) | Resume::Throw(value) | Resume::Return(value) => value.clone(),
        };
        let raising = !matches!(resume, Resume::Send(_));
        if generator.borrow().done {
            return Ok((Value::Undefined, true));
        }
        let depth = self.frames.len();
        let stack_base = self.stack.len();
        let (
            closure,
            locals,
            saved_stack,
            saved_handlers,
            instruction_pointer,
            this_value,
            new_target,
            arguments,
            started,
        ) = {
            let state = generator.borrow();
            (
                Rc::clone(&state.closure),
                state.locals.clone(),
                state.stack.clone(),
                state.handlers.clone(),
                state.instruction,
                state.this_value.clone(),
                state.new_target.clone(),
                state.arguments.clone(),
                state.started,
            )
        };

        self.frames.push(Frame {
            program: std::mem::replace(&mut self.program, Rc::clone(&closure.program)),
            function: self.function.take(),
            this_value: std::mem::replace(&mut self.this_value, this_value),
            new_target: std::mem::replace(&mut self.new_target, new_target),
            arguments: std::mem::replace(&mut self.arguments, arguments),
            constructing: None,
            locals: std::mem::replace(&mut self.locals, locals),
            instruction: self.instruction,
            stack_base,
        });
        self.function = Some(closure);
        self.instruction = instruction_pointer;
        self.stack.extend(saved_stack);
        // Re-base the generator's handlers onto wherever its stack landed this time.
        for (target, frame_offset, stack_offset) in saved_handlers {
            self.handlers.push(Handler {
                target,
                frame_depth: depth + 1 + frame_offset,
                stack_height: stack_base + stack_offset,
                native_depth: self.native_depth,
            });
        }
        if started && !raising {
            // The value the caller sent becomes the result of the `yield` that suspended. A
            // `.return()` does not resume the yield at all, so it pushes nothing.
            self.stack.push(sent.clone());
        }

        self.suspended = false;
        let outcome = if raising {
            // Throwing at the suspension point runs every `finally` between there and the top of
            // the generator, then arrives as `GeneratorReturned`.
            let raised = match &resume {
                Resume::Return(_) => Value::Native(Native::ReturnSignal),
                // A rejection is raised as itself, so the generator's own `catch` can see it.
                other => match other {
                    Resume::Throw(value) => value.clone(),
                    Resume::Send(_) => unreachable!("checked by `raising`"),
                    Resume::Return(_) => unreachable!("matched above"),
                },
            };
            match self
                .unwind(raised, instruction)
                .and_then(|()| self.run_frames(depth))
            {
                Err(ExecutionError::GeneratorReturned) => {
                    // The generator's own operand stack is abandoned along with it; without
                    // this the caller inherits whatever it had part-way through an expression.
                    self.stack.truncate(stack_base);
                    self.restore_caller_frame();
                    generator.borrow_mut().done = true;
                    return Ok((sent.clone(), true));
                }
                other => other?,
            }
        } else {
            self.run_frames(depth)?
        };

        if self.suspended {
            // Take back the handlers belonging to the generator, as offsets.
            let mut retained = Vec::new();
            while self
                .handlers
                .last()
                .is_some_and(|handler| handler.frame_depth > depth)
            {
                let handler = self.handlers.pop().expect("checked by the loop condition");
                retained.push((
                    handler.target,
                    handler.frame_depth - depth - 1,
                    handler.stack_height.saturating_sub(stack_base),
                ));
            }
            retained.reverse();

            let mut state = generator.borrow_mut();
            state.locals = std::mem::take(&mut self.locals);
            state.instruction = self.instruction;
            state.stack = self.stack.split_off(stack_base);
            state.handlers = retained;
            state.started = true;
            state.awaiting = self.suspended_awaiting;
            drop(state);

            self.restore_caller_frame();
            self.suspended = false;
            return Ok((outcome.value, false));
        }

        // A normal return finished the generator; `run_frames` already restored the caller.
        generator.borrow_mut().done = true;
        let _ = instruction;
        Ok((outcome.value, true))
    }

    /// Put back the frame a generator resumption displaced.
    fn restore_caller_frame(&mut self) {
        let frame = self
            .frames
            .pop()
            .expect("a resumption always pushes a frame");
        self.locals = frame.locals;
        self.program = frame.program;
        self.function = frame.function;
        self.this_value = frame.this_value;
        self.new_target = frame.new_target;
        self.arguments = frame.arguments;
        self.instruction = frame.instruction;
    }

    /// Collect every value an iterable yields.
    ///
    /// Drives the same protocol `for-of` uses rather than a second one, so anything iterable in
    /// a loop is iterable in a spread, and a value that is not iterable fails the same way.
    /// Invoke a native, including the few that need the interpreter itself.
    ///
    /// `call_native` is a free function so that the great majority of built-ins -- pure
    /// transformations of their arguments -- stay testable without a VM around them. The handful
    /// that drive the machine, by walking the iterator protocol or calling back into user code,
    /// cannot live there. They are peeled off here, and every call site goes through this, which
    /// is what stops the two halves from disagreeing about which native is which.
    fn dispatch_native(
        &mut self,
        native: Native,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        // A built-in that inspects a value's *shape* rather than its properties has no trap to
        // consult, so it looks through a proxy to what is being proxied. This is the honest
        // reading of "the trap is absent": the operation falls through to the target, which is
        // what every trapped operation does too.
        if matches!(arguments.first(), Some(Value::Proxy(_)))
            && matches!(
                native,
                Native::ObjectIsFrozen
                    | Native::ObjectIsExtensible
                    | Native::ObjectIsSealed
                    | Native::ObjectFreeze
                    | Native::ObjectSeal
                    | Native::ObjectPreventExtensions
                    | Native::ObjectGetPrototypeOf
                    | Native::ObjectAssign
                    // Serialising walks the value without the machine, so it cannot run a `get`
                    // trap. Unwrapping the outermost proxy makes the common case -- a wrapper
                    // whose traps only observe -- work; a proxy *nested* inside is still
                    // refused rather than serialised as whatever the target holds.
                    | Native::JsonStringify
            )
        {
            let mut unwrapped = arguments.to_vec();
            unwrapped[0] = unwrap_proxy(&arguments[0]).clone();
            return self.dispatch_native(native, receiver, &unwrapped, instruction);
        }
        match native {
            Native::ArrayFrom => self.array_from(arguments, instruction),
            // Built on `Array.from`, so an iterable, an array-like and a mapping function all
            // behave the same way they do there.
            Native::TypedArrayFrom(kind) => {
                let collected = self.array_from(arguments, instruction)?;
                Ok(construct_typed_array(kind, &[collected]))
            }
            // `Math.random` advances state that lives on the machine, so it cannot be a pure
            // function of its arguments either.
            Native::MathRandom => Ok(Value::Number(random_double(
                &mut self.realm.random_state.borrow_mut(),
            ))),
            // The scheduling built-ins are the event loop's only entry points, and the loop lives
            // on the machine, so they cannot be pure functions of their arguments.
            Native::SetTimeout | Native::SetInterval => {
                let delay = match arguments.get(1).map(Value::as_number) {
                    // A negative, missing or NaN delay is zero: "as soon as possible", which is
                    // still after everything already queued.
                    Some(delay) if delay.is_finite() && delay > 0.0 => delay,
                    _ => 0.0,
                };
                let id = {
                    let mut next = self.realm.next_timer_id.borrow_mut();
                    let id = *next;
                    *next += 1.0;
                    id
                };
                let sequence = {
                    let mut next = self.realm.next_sequence.borrow_mut();
                    *next += 1;
                    *next
                };
                self.realm.timers.borrow_mut().push(Timer {
                    id,
                    due: *self.realm.now.borrow() + delay,
                    sequence,
                    callback: arguments.first().cloned().unwrap_or(Value::Undefined),
                    // Everything after the delay is handed to the callback, which is how a
                    // timer carries state without a closure.
                    arguments: arguments.get(2..).unwrap_or_default().to_vec(),
                    repeat: matches!(native, Native::SetInterval).then_some(delay),
                });
                Ok(Value::Number(id))
            }
            Native::ClearTimeout | Native::ClearInterval => {
                let id = argument_number(arguments);
                self.realm
                    .timers
                    .borrow_mut()
                    .retain(|timer| timer.id != id);
                Ok(Value::Undefined)
            }
            Native::PromiseResolve => {
                // Already a promise? Hand it straight back; wrapping would add a tick for
                // nothing and break `Promise.resolve(p) === p`.
                let value = arguments.first().cloned().unwrap_or(Value::Undefined);
                if matches!(value, Value::Promise(_)) {
                    return Ok(value);
                }
                let promise = PromiseState::pending();
                self.resolve_promise(&promise, value);
                Ok(Value::Promise(promise))
            }
            Native::PromiseReject => {
                let promise = PromiseState::pending();
                let reason = arguments.first().cloned().unwrap_or(Value::Undefined);
                self.settle(&promise, &Settled::Rejected(reason));
                Ok(Value::Promise(promise))
            }
            Native::PromiseThen => {
                let Value::Promise(promise) = receiver else {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: receiver.type_name(),
                    });
                };
                let derived = PromiseState::pending();
                self.react(
                    &Rc::clone(promise),
                    arguments.first().cloned().unwrap_or(Value::Undefined),
                    arguments.get(1).cloned().unwrap_or(Value::Undefined),
                    Sink::Promise(Rc::clone(&derived)),
                    false,
                );
                Ok(Value::Promise(derived))
            }
            // `catch(r)` is `then(undefined, r)`, and saying so keeps one implementation.
            Native::PromiseCatch => self.dispatch_native(
                Native::PromiseThen,
                receiver,
                &[
                    Value::Undefined,
                    arguments.first().cloned().unwrap_or(Value::Undefined),
                ],
                instruction,
            ),
            // `finally(f)` runs `f` either way and passes the outcome through untouched, which
            // is what separates it from `then(f, f)`.
            Native::PromiseFinally => {
                let Value::Promise(promise) = receiver else {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: receiver.type_name(),
                    });
                };
                let handler = arguments.first().cloned().unwrap_or(Value::Undefined);
                let derived = PromiseState::pending();
                self.react(
                    &Rc::clone(promise),
                    handler.clone(),
                    handler,
                    Sink::Promise(Rc::clone(&derived)),
                    true,
                );
                Ok(Value::Promise(derived))
            }
            // `all`, `allSettled`, `race` and `any` differ only in when they settle and what
            // they collect, so they share one walk over the inputs.
            Native::PromiseAll
            | Native::PromiseAllSettled
            | Native::PromiseRace
            | Native::PromiseAny => {
                let source = arguments.first().cloned().unwrap_or(Value::Undefined);
                let items = self.drain_iterable(&source, instruction)?;
                self.combine(native, items)
            }
            Native::StringConstructor => {
                let Some(value) = arguments.first() else {
                    return Ok(Value::String(String::new()));
                };
                // A string hint, because that is what `String(x)` asks for -- so an object with
                // both `valueOf` and `toString` uses the second.
                let primitive = self.as_primitive(value, true, instruction)?;
                Ok(Value::String(primitive.to_display_string()))
            }
            Native::JsonStringify => {
                let indent = json_indent(arguments.get(2));
                let value = arguments.first().cloned().unwrap_or(Value::Undefined);
                let mut seen = Vec::new();
                let mut resolve = |getter: Option<&Value>, receiver: &Value| match getter {
                    Some(getter) => {
                        self.call_value(getter, receiver.clone(), Vec::new(), instruction)
                    }
                    None => {
                        // Only a reference value can carry one, and asking a primitive would
                        // reach the string and number method tables for nothing.
                        if !matches!(
                            receiver,
                            Value::Object(_) | Value::Array(_) | Value::Date(_) | Value::Proxy(_)
                        ) {
                            return Ok(receiver.clone());
                        }
                        let method = self.read_property(receiver, "toJSON", instruction)?;
                        if !is_callable(&method) {
                            return Ok(receiver.clone());
                        }
                        self.call_value(&method, receiver.clone(), Vec::new(), instruction)
                    }
                };
                Ok(
                    match json_stringify(&value, &indent, "", &mut seen, &mut resolve)? {
                        Some(text) => Value::String(text),
                        // `JSON.stringify(undefined)` is `undefined`, not the string.
                        None => Value::Undefined,
                    },
                )
            }
            // The *virtual* clock, not the host's. A program that asks the time twice with no
            // timer in between gets the same answer, which is what makes a run repeatable --
            // and is the whole point of the clock being the machine's. ADR 0015.
            // The same virtual clock `Date.now` reads, so a program that measures with one and
            // reports with the other stays consistent.
            Native::DateNow | Native::PerformanceNow => Ok(Value::Number(*self.realm.now.borrow())),
            // Copying reads each property, and a read is a call when the property is an
            // accessor -- which is why this cannot be a pure transformation of its arguments.
            Native::ObjectAssign => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                for source in arguments.iter().skip(1) {
                    for (key, property) in self.enumerable_own_properties(source, instruction)? {
                        let value = match property {
                            Property::Value(value) => value,
                            Property::Accessor { get, .. } => match get {
                                Some(getter) => self.call_value(
                                    &getter,
                                    source.clone(),
                                    Vec::new(),
                                    instruction,
                                )?,
                                None => continue,
                            },
                        };
                        self.set_computed(&target, &key_value(&key), value, instruction)?;
                    }
                }
                Ok(target)
            }
            Native::ObjectEntries | Native::ObjectValues | Native::ReflectOwnKeys => {
                let source = arguments.first().cloned().unwrap_or(Value::Undefined);
                // A proxy answers "what keys do you have" with a trap, not with a property
                // store, so the walk below has nothing to read.
                if matches!(source, Value::Proxy(_)) {
                    let mut items = Vec::new();
                    for key in self.own_keys(&source, instruction)? {
                        if !matches!(native, Native::ReflectOwnKeys)
                            && !self.proxy_property_is_enumerable(&source, &key, instruction)?
                        {
                            continue;
                        }
                        items.push(match native {
                            Native::ReflectOwnKeys => key_value(&key),
                            Native::ObjectValues => {
                                if !matches!(key, PropertyKey::String(_)) {
                                    continue;
                                }
                                self.read_property(&source, key.clone(), instruction)?
                            }
                            _ => Value::Array(ArrayData::new(vec![
                                key_value(&key),
                                self.read_property(&source, key.clone(), instruction)?,
                            ])),
                        });
                    }
                    return Ok(Value::Array(ArrayData::new(items)));
                }
                let mut items = Vec::new();
                let properties = if matches!(native, Native::ReflectOwnKeys) {
                    own_property_entries(&source)
                } else {
                    own_properties(&source)
                };
                for (key, property) in properties {
                    if !matches!(key, PropertyKey::String(_))
                        && !matches!(native, Native::ReflectOwnKeys)
                    {
                        continue;
                    }
                    if matches!(native, Native::ReflectOwnKeys) {
                        items.push(key_value(&key));
                        continue;
                    }
                    let value = match property {
                        Property::Value(value) => value,
                        Property::Accessor { get, .. } => match get {
                            Some(getter) => {
                                self.call_value(&getter, source.clone(), Vec::new(), instruction)?
                            }
                            None => Value::Undefined,
                        },
                    };
                    items.push(if matches!(native, Native::ObjectValues) {
                        value
                    } else {
                        Value::Array(ArrayData::new(vec![key_value(&key), value]))
                    });
                }
                Ok(Value::Array(ArrayData::new(items)))
            }
            Native::ObjectFromEntries => {
                let source = arguments.first().cloned().unwrap_or(Value::Undefined);
                let mut data = ObjectData::default();
                for entry in self.drain_iterable(&source, instruction)? {
                    let key = self.computed(&entry, &Value::Number(0.0));
                    let value = self.computed(&entry, &Value::Number(1.0));
                    data.set(key.to_property_key(), value);
                }
                Ok(Value::Object(Rc::new(RefCell::new(data))))
            }
            // `String.raw` is the one built-in written to be used as a template tag: it reads
            // the `raw` half of the template object, which is the text before escapes were
            // interpreted.
            Native::StringRaw => {
                let strings = arguments.first().cloned().unwrap_or(Value::Undefined);
                let raw = self.read_property(&strings, "raw", instruction)?;
                // An array's elements are not named properties, so an index read has to go
                // through the array itself; anything else is treated as array-like.
                let segments = match &raw {
                    Value::Array(items) => dense_items(&items.borrow()),
                    other => {
                        let length = self
                            .read_property(other, "length", instruction)?
                            .as_number();
                        let length = if length.is_finite() {
                            length.max(0.0) as usize
                        } else {
                            0
                        };
                        let mut segments = Vec::with_capacity(length);
                        for index in 0..length {
                            segments.push(self.read_property(
                                other,
                                index.to_string(),
                                instruction,
                            )?);
                        }
                        segments
                    }
                };
                let mut out = String::new();
                for (index, segment) in segments.iter().enumerate() {
                    out.push_str(&segment.to_display_string());
                    // One substitution between each pair of segments, and none after the last.
                    if index + 1 < segments.len()
                        && let Some(value) = arguments.get(index + 1)
                    {
                        out.push_str(&value.to_display_string());
                    }
                }
                Ok(Value::String(out))
            }
            // `Object.prototype.hasOwnProperty.call(proxy, k)` is how a library asks a value
            // whether it really holds a key, and on a proxy the answer is a trap: the pure
            // implementation reads a property store and would say no to everything.
            Native::ObjectHasOwnProperty if matches!(receiver, Value::Proxy(_)) => {
                let key = arguments.first().cloned().unwrap_or(Value::Undefined);
                let described = self.proxy_descriptor(receiver, &key, instruction)?;
                Ok(Value::Boolean(!matches!(
                    described,
                    Value::Undefined | Value::Null
                )))
            }
            Native::ObjectHasOwn if matches!(arguments.first(), Some(Value::Proxy(_))) => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let key = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let described = self.proxy_descriptor(&target, &key, instruction)?;
                Ok(Value::Boolean(!matches!(
                    described,
                    Value::Undefined | Value::Null
                )))
            }
            Native::ObjectGetOwnPropertySymbols
                if matches!(arguments.first(), Some(Value::Proxy(_))) =>
            {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let symbols = self
                    .own_keys(&target, instruction)?
                    .into_iter()
                    .filter_map(|key| match key {
                        PropertyKey::Symbol(symbol) => Some(Value::Symbol(symbol)),
                        _ => None,
                    })
                    .collect();
                Ok(Value::Array(ArrayData::new(symbols)))
            }
            Native::ObjectOwnNames if matches!(arguments.first(), Some(Value::Proxy(_))) => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                Ok(Value::Array(ArrayData::new(
                    self.own_keys(&target, instruction)?
                        .into_iter()
                        .filter_map(|key| string_key(&key).map(str::to_owned).map(Value::String))
                        .collect(),
                )))
            }
            Native::ObjectPropertyIsEnumerable if matches!(receiver, Value::Proxy(_)) => {
                let key = arguments.first().map_or_else(
                    || PropertyKey::String(String::new()),
                    Value::to_property_key,
                );
                Ok(Value::Boolean(self.proxy_property_is_enumerable(
                    receiver,
                    &key,
                    instruction,
                )?))
            }
            // Describing and defining a property are trapped operations of their own. A library
            // that copies a proxy into a plain object -- which is what a copy-on-write layer
            // does when it finalises -- goes through exactly this pair.
            Native::ObjectGetOwnPropertyDescriptor
                if matches!(arguments.first(), Some(Value::Proxy(_))) =>
            {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let key = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                self.proxy_descriptor(&target, &key, instruction)
            }
            Native::ObjectGetOwnPropertyDescriptors
                if matches!(arguments.first(), Some(Value::Proxy(_))) =>
            {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let mut data = ObjectData::default();
                for key in self.own_keys(&target, instruction)? {
                    let descriptor =
                        self.proxy_descriptor(&target, &key_value(&key), instruction)?;
                    if !matches!(descriptor, Value::Undefined) {
                        data.set(&key, descriptor);
                    }
                }
                Ok(Value::Object(Rc::new(RefCell::new(data))))
            }
            Native::ObjectDefineProperty if matches!(arguments.first(), Some(Value::Proxy(_))) => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let Value::Proxy(proxy) = &target else {
                    unreachable!("checked by the guard");
                };
                let proxy = Rc::clone(proxy);
                let inner = proxy.target.clone();
                let key = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let descriptor = arguments.get(2).cloned().unwrap_or(Value::Undefined);
                match self.proxy_trap(&proxy, "defineProperty", instruction)? {
                    Some(trap) => {
                        let handler = proxy.handler.clone();
                        let list = vec![inner, Self::trap_key(&key), descriptor];
                        self.call_value(&trap, handler, list, instruction)?;
                    }
                    None => {
                        self.dispatch_native(
                            native,
                            receiver,
                            &[inner, key, descriptor],
                            instruction,
                        )?;
                    }
                }
                Ok(target)
            }
            // `Object.keys` on a proxy is the `ownKeys` trap; the pure implementation reads a
            // property store the proxy does not have.
            Native::ObjectKeys if matches!(arguments.first(), Some(Value::Proxy(_))) => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let keys = self.own_keys(&target, instruction)?;
                let mut enumerable = Vec::new();
                for key in keys {
                    if self.proxy_property_is_enumerable(&target, &key, instruction)?
                        && let Some(name) = string_key(&key)
                    {
                        enumerable.push(Value::String(name.to_owned()));
                    }
                }
                Ok(Value::Array(ArrayData::new(enumerable)))
            }
            // `map.forEach(cb)` calls back into the program, so it belongs here rather than in
            // the pure table. A Set passes its value as both arguments, which is the same shape
            // its `entries` takes and the reason one callback signature serves both.
            Native::CollectionForEach => {
                let Some((entries, is_map)) = collection_state(receiver) else {
                    return Ok(Value::Undefined);
                };
                let callback = arguments.first().cloned().unwrap_or(Value::Undefined);
                let this_value = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let mut index = 0;
                // The length is re-read each step: entries added while iterating are visited,
                // which is what a Set does and is observable.
                loop {
                    let entry = entries.borrow().get(index).cloned();
                    let Some(entry) = entry else { break };
                    index += 1;
                    if matches!(entry, Value::Native(Native::Tombstone)) {
                        continue;
                    }
                    let (key, value) = match (is_map, &entry) {
                        (true, Value::Array(pair)) => {
                            let pair = pair.borrow();
                            (
                                pair.first().cloned().unwrap_or(Value::Undefined),
                                pair.get(1).cloned().unwrap_or(Value::Undefined),
                            )
                        }
                        _ => (entry.clone(), entry.clone()),
                    };
                    self.call_value(
                        &callback,
                        this_value.clone(),
                        vec![value, key, receiver.clone()],
                        instruction,
                    )?;
                }
                Ok(Value::Undefined)
            }
            Native::FunctionCall | Native::FunctionApply => {
                let this_value = arguments.first().cloned().unwrap_or(Value::Undefined);
                let rest = if matches!(native, Native::FunctionCall) {
                    arguments.get(1..).unwrap_or_default().to_vec()
                } else {
                    // `apply` takes one array-like. `null` and `undefined` mean no arguments,
                    // which is what a forwarding wrapper passes when it has none.
                    match arguments.get(1) {
                        Some(Value::Array(items)) => dense_items(&items.borrow()),
                        Some(Value::Undefined | Value::Null) | None => Vec::new(),
                        Some(other) => {
                            let length = self.read_property(other, "length", instruction)?;
                            let length = length.as_number();
                            let mut list = Vec::new();
                            for index in 0..(if length.is_finite() {
                                length.max(0.0) as usize
                            } else {
                                0
                            }) {
                                list.push(self.read_property(
                                    other,
                                    index.to_string(),
                                    instruction,
                                )?);
                            }
                            list
                        }
                    }
                };
                let callee = receiver.clone();
                self.call_value(&callee, this_value, rest, instruction)
            }
            Native::FunctionBind => Ok(Value::Bound(Rc::new(BoundFunction {
                target: receiver.clone(),
                this_value: arguments.first().cloned().unwrap_or(Value::Undefined),
                arguments: arguments.get(1..).unwrap_or_default().to_vec(),
            }))),
            // `Object.create(proto, descriptors)` is `Object.create(proto)` followed by
            // `Object.defineProperties`. The descriptors may carry accessors, and an accessor
            // is a function -- so the second argument cannot be handled beside the first one,
            // which needs no machine at all.
            Native::ObjectCreate if matches!(arguments.get(1), Some(value) if !value.is_nullish()) =>
            {
                let object = call_native(Native::ObjectCreate, receiver, &arguments[..1])?;
                self.dispatch_native(
                    Native::ObjectDefineProperties,
                    &Value::Undefined,
                    &[object.clone(), arguments[1].clone()],
                    instruction,
                )?;
                Ok(object)
            }
            Native::ObjectDefineProperties => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let source = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                for key in own_property_keys(&source) {
                    let descriptor = self.read_property(&source, &key, instruction)?;
                    self.dispatch_native(
                        Native::ObjectDefineProperty,
                        &Value::Undefined,
                        &[target.clone(), key_value(&key), descriptor],
                        instruction,
                    )?;
                }
                Ok(target)
            }
            Native::ObjectDefineProperty => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let key = arguments.get(1).map_or_else(
                    || PropertyKey::String(String::new()),
                    Value::to_property_key,
                );
                let descriptor = arguments.get(2).cloned().unwrap_or(Value::Undefined);
                let had_own_property = own_property_exists(&target, &key_value(&key));
                let enumerable_present =
                    has_property(&descriptor, &Value::String("enumerable".to_owned()));
                let enumerable = self
                    .read_property(&descriptor, "enumerable", instruction)?
                    .is_truthy();
                let value_present = has_property(&descriptor, &Value::String("value".to_owned()));
                let getter_present = has_property(&descriptor, &Value::String("get".to_owned()));
                let setter_present = has_property(&descriptor, &Value::String("set".to_owned()));
                let existing = match &target {
                    Value::Object(object) => object.borrow().get_own_property(key.clone()).cloned(),
                    Value::Function(closure) => closure
                        .properties
                        .borrow()
                        .get_own_property(key.clone())
                        .cloned(),
                    Value::Array(items) => {
                        match string_key(&key).and_then(|key| key.parse::<usize>().ok()) {
                            Some(index) => items.borrow().get(index).and_then(|value| {
                                (!matches!(value, Value::Native(Native::Hole)))
                                    .then(|| Property::Value(value.clone()))
                            }),
                            None => items
                                .properties
                                .borrow()
                                .get_own_property(key.clone())
                                .cloned(),
                        }
                    }
                    _ => None,
                };
                let getter = self.read_property(&descriptor, "get", instruction)?;
                let setter = self.read_property(&descriptor, "set", instruction)?;
                if getter_present || setter_present {
                    let (old_getter, old_setter) = match existing {
                        Some(Property::Accessor { get, set }) => (get, set),
                        _ => (None, None),
                    };
                    let halves = (
                        if getter_present {
                            is_callable(&getter).then_some(getter)
                        } else {
                            old_getter
                        },
                        if setter_present {
                            is_callable(&setter).then_some(setter)
                        } else {
                            old_setter
                        },
                    );
                    match &target {
                        Value::Object(object) => {
                            object
                                .borrow_mut()
                                .define_accessor(&key, halves.0, halves.1);
                        }
                        Value::Function(closure) => {
                            closure
                                .properties
                                .borrow_mut()
                                .define_accessor(&key, halves.0, halves.1);
                        }
                        _ => {}
                    }
                } else if !value_present && existing.is_some() {
                    // A partial descriptor such as `{ enumerable: false }` changes only the
                    // requested attributes. In particular, it must not turn an existing
                    // accessor into a data property whose value is `undefined`.
                    if let Value::Array(items) = &target
                        && let Some(index) =
                            string_key(&key).and_then(|key| key.parse::<usize>().ok())
                        && enumerable_present
                        && !items.properties.borrow().is_frozen()
                    {
                        items.set_index_enumerable(index, enumerable);
                    }
                } else {
                    let value = if value_present {
                        self.read_property(&descriptor, "value", instruction)?
                    } else {
                        match existing {
                            Some(Property::Value(value)) => value,
                            _ => Value::Undefined,
                        }
                    };
                    // An own property, not an assignment: see `ObjectData::define_value`.
                    match &target {
                        Value::Object(object) => object.borrow_mut().define_value(&key, value),
                        Value::Function(closure) => {
                            closure.properties.borrow_mut().define_value(&key, value);
                        }
                        Value::Array(items) => {
                            match string_key(&key).and_then(|key| key.parse::<usize>().ok()) {
                                Some(index) => {
                                    let mut items = items.borrow_mut();
                                    if index >= items.len() {
                                        items.resize(index + 1, Value::Native(Native::Hole));
                                    }
                                    items[index] = value;
                                }
                                None => items.properties.borrow_mut().define_value(&key, value),
                            }
                        }
                        // A primitive has nowhere to put one; the specification throws, and
                        // there is no more information in doing so here.
                        _ => {}
                    }
                }
                if enumerable_present || !had_own_property {
                    set_target_enumerable(&target, &key, enumerable);
                }
                Ok(target)
            }
            Native::ReflectGet => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let key = arguments.get(1).map_or_else(
                    || PropertyKey::String(String::new()),
                    Value::to_property_key,
                );
                self.read_property(&target, &key, instruction)
            }
            Native::ReflectSet => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let key = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let value = arguments.get(2).cloned().unwrap_or(Value::Undefined);
                self.set_computed(&target, &key, value, instruction)?;
                Ok(Value::Boolean(true))
            }
            Native::ReflectApply => {
                let callee = arguments.first().cloned().unwrap_or(Value::Undefined);
                let this_value = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let list = match arguments.get(2) {
                    Some(Value::Array(items)) => dense_items(&items.borrow()),
                    _ => Vec::new(),
                };
                self.call_value(&callee, this_value, list, instruction)
            }
            Native::SymbolFor => {
                let key = arguments
                    .first()
                    .map_or_else(String::new, Value::to_display_string);
                if let Some((_, symbol)) = self
                    .realm
                    .symbol_registry
                    .borrow()
                    .iter()
                    .find(|(name, _)| *name == key)
                {
                    return Ok(symbol.clone());
                }
                let mut serial = self.realm.symbol_serial.borrow_mut();
                *serial += 1;
                let symbol = Value::Symbol(Symbol::Unique(Rc::new(UniqueSymbol {
                    serial: *serial,
                    description: Some(key.clone()),
                })));
                self.realm
                    .symbol_registry
                    .borrow_mut()
                    .push((key, symbol.clone()));
                Ok(symbol)
            }
            Native::SymbolKeyFor => {
                let Some(Value::Symbol(symbol)) = arguments.first() else {
                    return Ok(Value::Undefined);
                };
                // Only a registered symbol has a key; one from `Symbol()` does not, which is how
                // a program tells the two apart.
                Ok(self
                    .realm
                    .symbol_registry
                    .borrow()
                    .iter()
                    .find(|(_, registered)| {
                        matches!(registered, Value::Symbol(other) if other == symbol)
                    })
                    .map_or(Value::Undefined, |(key, _)| Value::String(key.clone())))
            }
            Native::DateConstructor => {
                let time = match arguments.len() {
                    0 => *self.realm.now.borrow(),
                    1 => match arguments.first() {
                        // `new Date(otherDate)` copies the instant rather than stringifying it.
                        Some(Value::Date(existing)) => existing.get(),
                        Some(Value::String(text)) => parse_date(text),
                        Some(value) => {
                            let primitive = self.as_primitive(value, false, instruction)?;
                            match primitive {
                                Value::String(text) => parse_date(&text),
                                other => other.as_number(),
                            }
                        }
                        None => unreachable!("length checked above"),
                    },
                    _ => date_from_fields(arguments),
                };
                // Every instant outside the representable range collapses to one invalid date,
                // which is why the readers only have to test for NaN.
                let time = if time.abs() <= 8.64e15 {
                    time
                } else {
                    f64::NAN
                };
                Ok(Value::Date(Rc::new(std::cell::Cell::new(time))))
            }
            Native::RegExpConstructor => {
                let pattern = match arguments.first() {
                    Some(Value::RegExp(existing)) => existing.source.clone(),
                    None | Some(Value::Undefined) => String::new(),
                    Some(value) => value.to_display_string(),
                };
                let flags = match arguments.get(1) {
                    None | Some(Value::Undefined) => match arguments.first() {
                        // `new RegExp(re)` keeps the original's flags.
                        Some(Value::RegExp(existing)) => existing.flags.clone(),
                        _ => String::new(),
                    },
                    Some(value) => value.to_display_string(),
                };
                // An error rather than `throw_error` followed by a value: this function returns
                // one, and throwing *and* returning undefined pushed the undefined on top of the
                // handler's stack -- so `catch (e)` caught nothing rather than a SyntaxError.
                build_regexp(&pattern, &flags).ok_or(ExecutionError::InvalidRegExp)
            }
            Native::RegExpTest | Native::RegExpExec => {
                let Value::RegExp(regexp) = receiver else {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: receiver.type_name(),
                    });
                };
                let subject: Vec<u16> = arguments
                    .first()
                    .map_or_else(String::new, Value::to_display_string)
                    .encode_utf16()
                    .collect();
                // Only `g` and `y` carry a cursor; a plain pattern always searches from the
                // start, however many times it is used.
                let start = if regexp.global || regexp.sticky {
                    regexp.last_index.get()
                } else {
                    0
                };
                let found = regexp_search(regexp, &subject, start);
                if (regexp.global || regexp.sticky) && found.is_none() {
                    // A failed search resets the cursor, which is what stops a `g` regexp from
                    // being permanently stuck past the end.
                    regexp.last_index.set(0);
                }
                if let Some((range, _, _)) = &found
                    && (regexp.global || regexp.sticky)
                {
                    regexp.last_index.set(range.end);
                }
                if matches!(native, Native::RegExpTest) {
                    return Ok(Value::Boolean(found.is_some()));
                }
                let Some((range, groups, named)) = found else {
                    return Ok(Value::Null);
                };
                Ok(match_result(&subject, &range, &groups, &named))
            }
            Native::StringReplace | Native::StringReplaceAll
                if matches!(arguments.first(), Some(Value::RegExp(_)))
                    || matches!(
                        arguments.get(1),
                        Some(Value::Function(_) | Value::Native(_) | Value::Callback(_))
                    ) =>
            {
                self.replace_with_pattern(receiver, arguments, native, instruction)
            }
            Native::StringSplit if matches!(arguments.first(), Some(Value::RegExp(_))) => {
                let Value::String(text) = receiver else {
                    return Ok(Value::Undefined);
                };
                let Some(Value::RegExp(regexp)) = arguments.first() else {
                    unreachable!("checked by the guard");
                };
                let subject: Vec<u16> = text.encode_utf16().collect();
                let mut parts = Vec::new();
                let mut copied = 0;
                if subject.is_empty() {
                    // An empty subject is the one case with no trailing piece: a separator that
                    // matches it splits it into nothing, and one that does not leaves it whole.
                    if regexp_search(regexp, &subject, 0).is_none() {
                        parts.push(Value::String(String::new()));
                    }
                    return Ok(Value::Array(ArrayData::new(parts)));
                }
                for (range, groups, _) in regexp_search_all(regexp, &subject) {
                    // A separator that matches nothing would split between every character
                    // *and* produce an empty piece for each; the specification skips an empty
                    // match at the position the last piece started.
                    if range.start == range.end && range.start == copied {
                        continue;
                    }
                    // A match *at* the end is never a separator: the walk stops before the last
                    // position, and the piece after the final separator is pushed below. Without
                    // this, `"abc".split(/x*/)` grew a fourth, empty part.
                    if range.start == subject.len() {
                        continue;
                    }
                    parts.push(Value::String(utf16_slice(&subject, copied..range.start)));
                    // A capturing separator contributes its groups to the result, which is what
                    // makes `"a1b".split(/(\d)/)` three elements rather than two.
                    for group in groups.iter().skip(1) {
                        parts.push(match group {
                            Some(range) => Value::String(utf16_slice(&subject, range.clone())),
                            None => Value::Undefined,
                        });
                    }
                    copied = range.end;
                }
                parts.push(Value::String(utf16_slice(&subject, copied..subject.len())));
                Ok(Value::Array(ArrayData::new(parts)))
            }
            Native::StringSearch => {
                let Value::String(text) = receiver else {
                    return Ok(Value::Number(-1.0));
                };
                let subject: Vec<u16> = text.encode_utf16().collect();
                let Some(regexp) = self.pattern_argument(arguments, instruction)? else {
                    return Ok(Value::Number(-1.0));
                };
                let Value::RegExp(regexp) = &regexp else {
                    return Ok(Value::Number(-1.0));
                };
                // `search` ignores `lastIndex` entirely and always scans from the start.
                Ok(Value::Number(
                    regexp_search(regexp, &subject, 0)
                        .map_or(-1.0, |(range, _, _)| range.start as f64),
                ))
            }
            Native::StringMatch | Native::StringMatchAll => {
                let Value::String(text) = receiver else {
                    return Ok(Value::Null);
                };
                let subject: Vec<u16> = text.encode_utf16().collect();
                let Some(pattern) = self.pattern_argument(arguments, instruction)? else {
                    return Ok(Value::Null);
                };
                let Value::RegExp(regexp) = &pattern else {
                    return Ok(Value::Null);
                };
                let all = matches!(native, Native::StringMatchAll) || regexp.global;
                if !all {
                    // Without `g`, `match` is `exec`: one result with its groups.
                    return Ok(match regexp_search(regexp, &subject, 0) {
                        Some((range, groups, named)) => {
                            match_result(&subject, &range, &groups, &named)
                        }
                        None => Value::Null,
                    });
                }
                let mut results = Vec::new();
                for (range, groups, named) in regexp_search_all(regexp, &subject) {
                    results.push(if matches!(native, Native::StringMatchAll) {
                        // `matchAll` yields full results; `match` with `g` yields only the text.
                        match_result(&subject, &range, &groups, &named)
                    } else {
                        Value::String(utf16_slice(&subject, range))
                    });
                }
                if results.is_empty() && matches!(native, Native::StringMatch) {
                    return Ok(Value::Null);
                }
                regexp.last_index.set(0);
                Ok(Value::Array(ArrayData::new(results)))
            }
            Native::QueueMicrotask => {
                self.realm.microtasks.borrow_mut().push_back(Job::Callback(
                    arguments.first().cloned().unwrap_or(Value::Undefined),
                    Vec::new(),
                ));
                Ok(Value::Undefined)
            }
            // Every call has to produce a symbol distinct from every other, so this needs the
            // counter rather than only its arguments.
            Native::SymbolConstructor => {
                *self.realm.symbol_serial.borrow_mut() += 1;
                Ok(Value::Symbol(Symbol::Unique(Rc::new(UniqueSymbol {
                    serial: *self.realm.symbol_serial.borrow(),
                    description: match arguments.first() {
                        None | Some(Value::Undefined) => None,
                        Some(value) => Some(value.to_display_string()),
                    },
                }))))
            }
            _ => call_native(native, receiver, arguments),
        }
    }

    /// `Array.from(source, mapFn?)`.
    ///
    /// Anything iterable is walked through the protocol. Anything else is treated as array-like
    /// and read by index up to its `length`, which is the other half of what the specification
    /// says and what makes `Array.from({ length: 2 })` two undefineds rather than an error.
    fn array_from(
        &mut self,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let source = arguments.first().cloned().unwrap_or(Value::Undefined);
        let map = arguments.get(1).cloned().unwrap_or(Value::Undefined);

        let mut items = match &source {
            Value::Array(_) => {
                let method = self.read_property(&source, iterator_key(), instruction)?;
                if matches!(method, Value::Native(Native::ArrayValues)) {
                    if let Value::Array(items) = &source {
                        dense_items(&items.borrow())
                    } else {
                        unreachable!()
                    }
                } else {
                    self.drain_iterable_with_method(&source, method, instruction)?
                }
            }
            other => {
                let method = self.read_property(other, iterator_key(), instruction)?;
                if method.is_nullish() {
                    let length = match self.property(other, "length") {
                        Value::Number(length) if length >= 1.0 => length as usize,
                        _ => 0,
                    };
                    (0..length)
                        .map(|index| self.computed(&source, &Value::Number(index as f64)))
                        .collect()
                } else {
                    self.drain_iterable_with_method(other, method, instruction)?
                }
            }
        };

        if !map.is_nullish() {
            let mut mapped = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                mapped.push(self.call_value(
                    &map,
                    Value::Undefined,
                    vec![item.clone(), Value::Number(index as f64)],
                    instruction,
                )?);
            }
            items = mapped;
        }
        Ok(Value::Array(ArrayData::new(items)))
    }

    fn drain_iterable(
        &mut self,
        value: &Value,
        instruction: usize,
    ) -> Result<Vec<Value>, ExecutionError> {
        let method = self.read_property(value, iterator_key(), instruction)?;
        self.drain_iterable_with_method(value, method, instruction)
    }

    fn drain_iterable_with_method(
        &mut self,
        value: &Value,
        method: Value,
        instruction: usize,
    ) -> Result<Vec<Value>, ExecutionError> {
        let iterator = self.call_value(&method, value.clone(), Vec::new(), instruction)?;

        let mut collected = Vec::new();
        loop {
            let next = self.property(&iterator, "next");
            let step = self.call_value(&next, iterator.clone(), Vec::new(), instruction)?;
            if self.property(&step, "done").is_truthy() {
                return Ok(collected);
            }
            collected.push(self.property(&step, "value"));
            if collected.len() > self.limits.max_steps as usize {
                return Err(ExecutionError::StepLimitExceeded {
                    steps: collected.len() as u64,
                    limit: self.limits.max_steps,
                });
            }
        }
    }

    /// Run an array method that takes a callback.
    ///
    /// The length is re-read each step, so an array mutated by the callback behaves the way
    /// JavaScript says: elements appended during the walk are not visited, and a shortened array
    /// stops early rather than reading past its end.
    fn array_callback(
        &mut self,
        method: CallbackMethod,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Array(items) = receiver else {
            return Ok(Value::Undefined);
        };
        let callback = arguments.first().cloned().unwrap_or(Value::Undefined);
        let this_value = arguments.get(1).cloned().unwrap_or(Value::Undefined);

        let mut collected = Vec::new();
        let mut index = 0usize;
        loop {
            let Some(item) = items.borrow().get(index).cloned() else {
                break;
            };
            // Every element-walking method skips a hole and visits an undefined. `map` still
            // has to *leave* one: its result has an entry per input position, so a skipped hole
            // that produced nothing would shorten the array and shift everything after it.
            if Self::is_hole(&item) {
                if matches!(method, CallbackMethod::Map) {
                    collected.push(Value::Native(Native::Hole));
                }
                index += 1;
                continue;
            }
            let result = self.call_value(
                &callback,
                this_value.clone(),
                vec![item.clone(), Value::Number(index as f64), receiver.clone()],
                instruction,
            )?;
            match method {
                // Dispatched before `array_callback` is reached; a generator method is not an
                // array walk.
                CallbackMethod::GeneratorNext
                | CallbackMethod::GeneratorReturn
                | CallbackMethod::AsyncGeneratorNext => {
                    unreachable!("generator methods are handled by `invoke`")
                }
                CallbackMethod::Reduce => {
                    unreachable!("`reduce` is handled by `array_reduce`")
                }
                CallbackMethod::Sort => unreachable!("`sort` is handled by `array_sort`"),
                CallbackMethod::FindIndex => {
                    if result.is_truthy() {
                        return Ok(Value::Number(index as f64));
                    }
                }
                CallbackMethod::Map => collected.push(result),
                CallbackMethod::ForEach => {}
                CallbackMethod::Filter => {
                    if result.is_truthy() {
                        collected.push(item);
                    }
                }
                CallbackMethod::Find => {
                    if result.is_truthy() {
                        return Ok(item);
                    }
                }
                CallbackMethod::Some => {
                    if result.is_truthy() {
                        return Ok(Value::Boolean(true));
                    }
                }
                CallbackMethod::Every => {
                    if !result.is_truthy() {
                        return Ok(Value::Boolean(false));
                    }
                }
                CallbackMethod::FlatMap => collected.push(result),
                // Walked backwards below rather than here, so reaching this is a dispatch bug.
                CallbackMethod::FindLast | CallbackMethod::FindLastIndex => {
                    unreachable!("the last-first walks are handled by `array_callback_backwards`")
                }
                CallbackMethod::ReduceRight => {
                    unreachable!("`reduceRight` is handled by `array_reduce`")
                }
            }
            index += 1;
        }

        Ok(match method {
            CallbackMethod::GeneratorNext
            | CallbackMethod::GeneratorReturn
            | CallbackMethod::AsyncGeneratorNext => {
                unreachable!("generator methods are handled by `invoke`")
            }
            CallbackMethod::Reduce | CallbackMethod::ReduceRight => {
                unreachable!("`reduce` is handled by `array_reduce`")
            }
            CallbackMethod::Sort => unreachable!("`sort` is handled by `array_sort`"),
            CallbackMethod::FindLast | CallbackMethod::FindLastIndex => {
                unreachable!("the last-first walks are handled by `array_callback_backwards`")
            }
            // One level, and only of what the callback produced -- `flatMap` does not flatten an
            // array that was already an element.
            CallbackMethod::FlatMap => Value::Array(ArrayData::new(flatten(&collected, 1))),
            // Not undefined: `findIndex` reports -1 for "nowhere", where `find` reports the
            // absence of a value.
            CallbackMethod::FindIndex => Value::Number(-1.0),
            CallbackMethod::Map | CallbackMethod::Filter => Value::Array(ArrayData::new(collected)),
            CallbackMethod::ForEach | CallbackMethod::Find => Value::Undefined,
            CallbackMethod::Some => Value::Boolean(false),
            CallbackMethod::Every => Value::Boolean(true),
        })
    }

    /// `reduce(callback, initial?)`.
    ///
    /// With no initial value the first element seeds the accumulator and the walk starts at the
    /// second, which is why this cannot share `array_callback`'s loop: the starting index depends
    /// on an argument, and the callback takes four parameters rather than three.
    /// `findLast` and `findLastIndex`, which walk from the end.
    ///
    /// A separate walk rather than a flag on the forward one: the length is re-read each step so a
    /// callback that shortens the array stops the walk, and "the step after the last one" is a
    /// different thing when counting down.
    fn array_callback_backwards(
        &mut self,
        method: CallbackMethod,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Array(items) = receiver else {
            return Ok(Value::Undefined);
        };
        let callback = arguments.first().cloned().unwrap_or(Value::Undefined);
        let this_value = arguments.get(1).cloned().unwrap_or(Value::Undefined);
        let mut index = items.borrow().len();
        while index > 0 {
            index -= 1;
            // A hole is visited as undefined here, unlike `find`: the last-first pair were added
            // to the language after the decision to stop skipping them.
            let item = items
                .borrow()
                .get(index)
                .cloned()
                .filter(|item| !Self::is_hole(item))
                .unwrap_or(Value::Undefined);
            let result = self.call_value(
                &callback,
                this_value.clone(),
                vec![item.clone(), Value::Number(index as f64), receiver.clone()],
                instruction,
            )?;
            if result.is_truthy() {
                return Ok(if matches!(method, CallbackMethod::FindLastIndex) {
                    Value::Number(index as f64)
                } else {
                    item
                });
            }
        }
        Ok(if matches!(method, CallbackMethod::FindLastIndex) {
            Value::Number(-1.0)
        } else {
            Value::Undefined
        })
    }

    fn array_reduce(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        backwards: bool,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Array(items) = receiver else {
            return Ok(Value::Undefined);
        };
        let callback = arguments.first().cloned().unwrap_or(Value::Undefined);
        let length = items.borrow().len();
        let (mut accumulator, mut index) = match arguments.get(1) {
            Some(initial) => (
                initial.clone(),
                if backwards { length.wrapping_sub(1) } else { 0 },
            ),
            None => {
                // Reducing an empty array with no initial value has no answer to give, which is
                // a TypeError in the specification and fatal here for want of a way to throw one.
                // The seed is the first element that is *there* from whichever end the walk
                // starts: an array of nothing but holes has no answer to give, exactly as an
                // empty one does not.
                let borrowed = items.borrow();
                let found = if backwards {
                    borrowed.iter().rposition(|item| !Self::is_hole(item))
                } else {
                    borrowed.iter().position(|item| !Self::is_hole(item))
                };
                let Some(at) = found else {
                    return Err(ExecutionError::EmptyReduce);
                };
                (
                    borrowed[at].clone(),
                    if backwards {
                        at.wrapping_sub(1)
                    } else {
                        at + 1
                    },
                )
            }
        };
        loop {
            // The length is re-read each step, so a callback that shortens the array stops the
            // walk rather than reading past the end -- the same rule `array_callback` follows.
            // Walking backwards, the step past the start wraps, which `get` reports the same way.
            let Some(item) = items.borrow().get(index).cloned() else {
                return Ok(accumulator);
            };
            // `reduce` walks elements, and a hole is not one: visiting it would fold undefined
            // into the accumulator and turn every sum over a sparse array into NaN.
            if Self::is_hole(&item) {
                index = if backwards {
                    index.wrapping_sub(1)
                } else {
                    index + 1
                };
                continue;
            }
            accumulator = self.call_value(
                &callback,
                Value::Undefined,
                vec![
                    accumulator,
                    item,
                    Value::Number(index as f64),
                    receiver.clone(),
                ],
                instruction,
            )?;
            index = if backwards {
                index.wrapping_sub(1)
            } else {
                index + 1
            };
        }
    }

    /// `sort(comparator?)`, in place.
    ///
    /// A bottom-up merge sort rather than `slice::sort_by`, for two reasons: the comparator can
    /// be user code, so each comparison has to be able to re-enter the interpreter, and the sort
    /// has to be stable, which the language requires and merging gives for free.
    ///
    /// Undefined elements sort to the end without ever reaching the comparator, and the default
    /// comparison is by string -- which is why `[10, 9, 1].sort()` is `[1, 10, 9]`.
    fn array_sort(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Array(items) = receiver else {
            return Ok(Value::Undefined);
        };
        let comparator = arguments.first().cloned().unwrap_or(Value::Undefined);
        let taken = std::mem::take(&mut *items.borrow_mut());
        let (mut present, absent): (Vec<_>, Vec<_>) = taken
            .into_iter()
            .partition(|item| !matches!(item, Value::Undefined));

        let mut width = 1;
        while width < present.len() {
            let mut merged = Vec::with_capacity(present.len());
            let mut start = 0;
            while start < present.len() {
                let middle = (start + width).min(present.len());
                let end = (start + 2 * width).min(present.len());
                let (mut left, mut right) = (start, middle);
                while left < middle && right < end {
                    // `<= 0` keeps the left run first on a tie, which is what makes it stable.
                    if self.compare_for_sort(
                        &comparator,
                        &present[left],
                        &present[right],
                        instruction,
                    )? <= 0.0
                    {
                        merged.push(present[left].clone());
                        left += 1;
                    } else {
                        merged.push(present[right].clone());
                        right += 1;
                    }
                }
                merged.extend_from_slice(&present[left..middle]);
                merged.extend_from_slice(&present[right..end]);
                start = end;
            }
            present = merged;
            width *= 2;
        }

        present.extend(absent);
        *items.borrow_mut() = present;
        // In place, and yields the same array rather than a copy.
        Ok(receiver.clone())
    }

    /// One comparison for `array_sort`: the comparator's number, or the default string order.
    fn compare_for_sort(
        &mut self,
        comparator: &Value,
        left: &Value,
        right: &Value,
        instruction: usize,
    ) -> Result<f64, ExecutionError> {
        if comparator.is_nullish() {
            let (left, right) = (left.to_display_string(), right.to_display_string());
            // UTF-16 code units, which is the order the language compares strings in.
            return Ok(match left.encode_utf16().cmp(right.encode_utf16()) {
                std::cmp::Ordering::Less => -1.0,
                std::cmp::Ordering::Equal => 0.0,
                std::cmp::Ordering::Greater => 1.0,
            });
        }
        let result = self.call_value(
            comparator,
            Value::Undefined,
            vec![left.clone(), right.clone()],
            instruction,
        )?;
        let ordering = result.as_number();
        // A comparator returning NaN is treated as "equal", which keeps the merge total rather
        // than letting an inconsistent comparator corrupt the result.
        Ok(if ordering.is_nan() { 0.0 } else { ordering })
    }

    /// Call `callee`, recovering the value it threw rather than unwinding past this point.
    ///
    /// The outer `Result` is the machine failing; the inner one is the program's own outcome --
    /// `Err` holding whatever it threw. A promise reaction needs exactly this shape, because a
    /// rejection carries a value and an `ExecutionError` carries only a message.
    fn call_catching(
        &mut self,
        callee: &Value,
        this_value: Value,
        arguments: Vec<Value>,
        instruction: usize,
    ) -> Result<Result<Value, Value>, ExecutionError> {
        let installed = self.handlers.len();
        self.handlers.push(Handler {
            target: HOST_HANDLER,
            frame_depth: self.frames.len(),
            stack_height: self.stack.len(),
            native_depth: self.native_depth,
        });
        match self.call_value(callee, this_value, arguments, instruction) {
            Ok(value) => {
                self.handlers.truncate(installed);
                Ok(Ok(value))
            }
            Err(ExecutionError::CaughtByHost) => {
                // `unwind` popped it; anything the callee left above it goes too.
                self.handlers.truncate(installed);
                Ok(Err(self.caught.take().unwrap_or(Value::Undefined)))
            }
            Err(error) => {
                self.handlers.truncate(installed);
                Err(error)
            }
        }
    }

    /// Call `callee` and run it to completion, returning its value.
    ///
    /// For a native this is a direct call; for a compiled function it pushes a frame and drives
    /// the machine until that frame returns. Built-ins that take a callback -- `map`, `filter`,
    /// `forEach` -- need exactly this, and having one place that does it keeps them from each
    /// inventing a way to re-enter the interpreter.
    fn call_value(
        &mut self,
        callee: &Value,
        this_value: Value,
        arguments: Vec<Value>,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        // Every arm below can re-enter the interpreter, and each re-entry is a Rust frame. The
        // count is taken here, once, rather than at each of them.
        if self.native_depth >= self.limits.max_native_depth {
            return Err(ExecutionError::CallDepthExceeded {
                limit: self.limits.max_native_depth,
            });
        }
        self.native_depth += 1;
        let outcome = self.call_value_inner(callee, this_value, arguments, instruction);
        self.native_depth -= 1;
        outcome
    }

    fn call_value_inner(
        &mut self,
        callee: &Value,
        this_value: Value,
        arguments: Vec<Value>,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        match callee {
            Value::Native(native) => {
                // A method that changes its array, given a proxy, has to change what is behind
                // the proxy: the snapshot it would otherwise work on is thrown away, and the
                // program's `push` would silently do nothing.
                if native.mutates_an_array() && matches!(this_value, Value::Proxy(_)) {
                    return self.mutate_through_proxy(
                        *native,
                        &this_value,
                        &arguments,
                        instruction,
                    );
                }
                let this_value = self.as_array_like(callee, this_value, instruction)?;
                self.dispatch_native(*native, &this_value, &arguments, instruction)
            }
            // `resolve` and `reject` from a promise executor. Calling one settles its promise
            // and evaluates to undefined, however many times the program calls it.
            Value::Settler(promise, rejects) => {
                let value = arguments.first().cloned().unwrap_or(Value::Undefined);
                let promise = Rc::clone(promise);
                if *rejects {
                    self.settle(&promise, &Settled::Rejected(value));
                } else {
                    self.resolve_promise(&promise, value);
                }
                Ok(Value::Undefined)
            }
            // A callback method reaches here when a built-in drives another built-in -- a spread
            // over a generator, for instance. `invoke` is the one place that knows how to run
            // each kind, so this defers to it rather than repeating the dispatch.
            Value::Callback(_) => {
                let this_value = self.as_array_like(callee, this_value, instruction)?;
                Ok(self
                    .invoke(callee.clone(), this_value, arguments, instruction)?
                    .unwrap_or(Value::Undefined))
            }
            // A proxied function runs its `apply` trap, or the target when there is none.
            Value::Proxy(proxy) => {
                let proxy = Rc::clone(proxy);
                let inner = proxy.target.clone();
                match self.proxy_trap(&proxy, "apply", instruction)? {
                    Some(trap) => {
                        let handler = proxy.handler.clone();
                        let list = Value::Array(ArrayData::new(arguments));
                        self.call_value(&trap, handler, vec![inner, this_value, list], instruction)
                    }
                    None => self.call_value(&inner, this_value, arguments, instruction),
                }
            }
            // Binding is transparent to the call: the receiver is the one `bind` fixed and the
            // bound arguments come first, whatever the target turns out to be.
            Value::Bound(bound) => {
                let mut all = bound.arguments.clone();
                all.extend(arguments);
                let target = bound.target.clone();
                let this_value = bound.this_value.clone();
                self.call_value(&target, this_value, all, instruction)
            }
            Value::Function(closure) => {
                let depth = self.frames.len();
                // The shared call path expects a callee beneath the arguments and pops it.
                self.stack.push(Value::Undefined);
                let argc = arguments.len();
                for argument in arguments {
                    self.stack.push(argument);
                }
                self.call_with_this(
                    Rc::clone(closure),
                    argc as u16,
                    instruction,
                    this_value,
                    None,
                )?;
                // Calling a generator function pushes no frame -- it leaves the generator object
                // on the stack and returns. There is nothing to drive, and driving anyway would
                // resume the *caller's* instruction stream, since `run_frames` runs until the
                // frame depth drops rather than checking it first.
                if self.frames.len() == depth {
                    return self.pop(instruction);
                }
                Ok(self.run_frames(depth)?.value)
            }
            other => Err(ExecutionError::NotCallable {
                instruction,
                value: other.type_name(),
            }),
        }
    }

    /// Read a module-scope slot from wherever the module body's locals currently live.
    ///
    /// At module level that is `self.locals`; inside any call it is the bottom frame's, because
    /// the module body is the bottom of the call stack. Going through the frame rather than
    /// copying module locals into each callee is what makes a module binding shared: an
    /// assignment from inside a function has to be visible to the next call that reads it.
    fn global(&self, slot: usize) -> Result<Value, ExecutionError> {
        self.module_locals()
            .get(slot)
            .cloned()
            .ok_or(ExecutionError::GlobalOutOfBounds { index: slot })
    }

    fn set_global(&mut self, slot: usize, value: Value) -> Result<(), ExecutionError> {
        let locals = match self.frames.first_mut() {
            Some(module) => &mut module.locals,
            None => &mut self.locals,
        };
        *locals
            .get_mut(slot)
            .ok_or(ExecutionError::GlobalOutOfBounds { index: slot })? = value;
        Ok(())
    }

    fn module_locals(&self) -> &[Value] {
        match self.frames.first() {
            Some(module) => &module.locals,
            None => &self.locals,
        }
    }

    /// Bytecode of the function currently executing.
    /// The instruction stream of the running frame, borrowed from `program` rather than `self`.
    ///
    /// Taking the `Rc` by value is what lets the interpreter loop hold an instruction across the
    /// `&mut self` calls that execute it: the borrow is on a local clone, not on the machine.
    fn code<'a>(program: &'a Rc<Program>, function: Option<&Rc<Closure>>) -> &'a [Instruction] {
        match function {
            None => &program.code,
            // Validated when the closure was created; a call never installs a bad index.
            Some(closure) => &program.functions[closure.function as usize].code,
        }
    }

    /// Run `parent` (on top of the stack, under `argc` arguments) against the current `this`.
    fn super_call(&mut self, argc: u16, current: usize) -> Result<(), ExecutionError> {
        let arguments = self.take_call_arguments(argc, current)?;
        let parent = self.pop(current)?;
        let this_value = self.this_value.clone();
        match parent {
            Value::Function(closure) => {
                // The parent initialises the *existing* instance rather than
                // allocating one, so this is an ordinary call with `this` threaded
                // through and no `constructing` slot.
                self.stack.push(Value::Undefined);
                let argc = arguments.len();
                for argument in arguments {
                    self.stack.push(argument);
                }
                // `super()` keeps the `new.target` of the call that is building the instance,
                // which is how a base constructor can name the subclass it is being built for.
                // `call_with_this` computes one from `constructing`, so it is put back after.
                let inherited = self.new_target.clone();
                self.call_with_this(closure, argc as u16, current, this_value, None)?;
                self.new_target = inherited;
            }
            // A native parent builds its own object, and `this` *becomes* that object rather
            // than being handed its properties. That is the whole of `class A extends Array`:
            // an array is not an ordinary object with elements copied onto it, it is an array.
            // It also replaces what used to be an Error-shaped special case here.
            Value::Native(parent) => {
                let Some(built) = self.construct_exotic(parent, &arguments, current)? else {
                    return Err(ExecutionError::NotConstructable {
                        instruction: current,
                        value: "native",
                    });
                };
                // A native parent leaves no frame, so the expression's value has to be pushed
                // here to match what a compiled parent's `Return` does.
                self.stack.push(Value::Undefined);

                // The derived class's prototype was put on the instance allocated up front; it
                // has to move to the object that is actually being returned, or the subclass's
                // methods would be unreachable and `instanceof A` would be false.
                let prototype = match &this_value {
                    Value::Object(instance) => instance.borrow().prototype().cloned(),
                    _ => None,
                };
                if let Some(prototype) = prototype {
                    match &built {
                        Value::Object(object) => object.borrow_mut().set_prototype(prototype),
                        Value::Array(array) => {
                            array.properties.borrow_mut().set_prototype(prototype);
                        }
                        _ => {}
                    }
                }

                // `this` is the parent's object from here on, and the frame's record of what is
                // being constructed has to follow -- otherwise `Return` yields the abandoned one.
                self.this_value = built.clone();
                if let Some(frame) = self.frames.last_mut() {
                    frame.constructing = Some(built);
                }
            }
            other => {
                return Err(ExecutionError::NotConstructable {
                    instruction: current,
                    value: other.type_name(),
                });
            }
        }
        Ok(())
    }

    /// The prototype for one error kind, created on first use.
    ///
    /// A subclass's prototype chains to `Error.prototype`, so `e instanceof Error` holds for
    /// every kind and `toString` is inherited rather than copied.
    fn error_prototype(&mut self, kind: ErrorKind) -> Value {
        if let Some(prototype) = &self.realm.error_prototypes.borrow()[kind.index()] {
            return prototype.clone();
        }
        let mut data = ObjectData::default();
        data.set("name", Value::String(kind.name().to_owned()));
        data.set("message", Value::String(String::new()));
        // `e.constructor.name` is how a program asks which kind it caught, and this is the link
        // that answers it.
        data.set("constructor", Value::Native(Native::Error(kind)));
        if matches!(kind, ErrorKind::Error) {
            // Marks the chain for `String(e)`. A real `toString` would be invoked through the
            // normal method path; this is the placeholder that keeps the observable behaviour
            // right until built-ins can be written in JavaScript (design.md §5).
            data.set("toString", Value::Native(Native::ErrorToString));
        }
        let prototype = Value::Object(Rc::new(RefCell::new(data)));
        if !matches!(kind, ErrorKind::Error) {
            let base = self.error_prototype(ErrorKind::Error);
            if let Value::Object(object) = &prototype {
                object.borrow_mut().set_prototype(base);
            }
        }
        self.realm.error_prototypes.borrow_mut()[kind.index()] = Some(prototype.clone());
        prototype
    }

    /// Run an async function's body until it awaits or finishes.
    ///
    /// The body is a generator: each `await` is a suspension, and this is what resumes it. The
    /// promise the caller holds is settled when the body returns or throws, so the whole of
    /// `async`/`await` is a generator plus this loop.
    fn drive_async(
        &mut self,
        generator: &Rc<RefCell<GeneratorState>>,
        promise: &Rc<RefCell<PromiseState>>,
        resume: Resume,
    ) -> Result<(), ExecutionError> {
        let instruction = self.instruction;
        match self.resume_catching(generator, resume, instruction)? {
            // The body threw and nothing in it caught: that is the function's rejection.
            Err(thrown) => self.settle(promise, &Settled::Rejected(thrown)),
            Ok((value, true)) => self.resolve_promise(promise, value),
            Ok((awaited, false)) => {
                // The same fast path `Promise.resolve` takes: an awaited promise is waited on
                // directly rather than wrapped in another. Wrapping would cost two extra ticks,
                // and `await` is one of the places where a tick is plainly observable.
                let target = match awaited {
                    Value::Promise(inner) => inner,
                    other => {
                        let pending = PromiseState::pending();
                        self.resolve_promise(&pending, other);
                        pending
                    }
                };
                self.react(
                    &target,
                    Value::Undefined,
                    Value::Undefined,
                    Sink::Async(Rc::clone(generator), Rc::clone(promise)),
                    false,
                );
            }
        }
        Ok(())
    }

    /// Advance an async generator one consumer step, settling `step` when it stops.
    ///
    /// An async generator suspends for two reasons. An `await` is invisible to the consumer: the
    /// body is waiting on something and the step is not finished, so this re-arms itself on the
    /// awaited promise. A `yield` is the step's result. That distinction is the whole of what
    /// separates this from [`Vm::drive_async`], which has only the first kind.
    fn drive_async_generator(
        &mut self,
        generator: &Rc<RefCell<GeneratorState>>,
        step: &Rc<RefCell<PromiseState>>,
        resume: Resume,
    ) -> Result<(), ExecutionError> {
        let instruction = self.instruction;
        match self.resume_catching(generator, resume, instruction)? {
            Err(thrown) => self.settle(step, &Settled::Rejected(thrown)),
            Ok((value, true)) => {
                let result = iteration_result(value, true);
                self.resolve_promise(step, result);
            }
            Ok((value, false)) if !generator.borrow().awaiting => {
                let result = iteration_result(value, false);
                self.resolve_promise(step, result);
            }
            Ok((awaited, false)) => {
                // Waiting, not yielding: the same fast path `await` takes in an async function.
                let target = match awaited {
                    Value::Promise(inner) => inner,
                    other => {
                        let pending = PromiseState::pending();
                        self.resolve_promise(&pending, other);
                        pending
                    }
                };
                self.react(
                    &target,
                    Value::Undefined,
                    Value::Undefined,
                    Sink::AsyncGenerator(Rc::clone(generator), Rc::clone(step)),
                    false,
                );
            }
        }
        Ok(())
    }

    /// Resume a generator, recovering what it threw rather than unwinding past this point.
    fn resume_catching(
        &mut self,
        generator: &Rc<RefCell<GeneratorState>>,
        resume: Resume,
        instruction: usize,
    ) -> Result<Result<(Value, bool), Value>, ExecutionError> {
        let installed = self.handlers.len();
        self.handlers.push(Handler {
            target: HOST_HANDLER,
            frame_depth: self.frames.len(),
            stack_height: self.stack.len(),
            native_depth: self.native_depth,
        });
        let outcome = self.resume_generator_with(generator, resume, instruction);
        self.handlers.truncate(installed);
        match outcome {
            Ok(value) => Ok(Ok(value)),
            Err(ExecutionError::CaughtByHost) => {
                Ok(Err(self.caught.take().unwrap_or(Value::Undefined)))
            }
            Err(error) => Err(error),
        }
    }

    /// Settle a promise, queueing whatever was waiting on it.
    ///
    /// Only the first settlement counts: a promise is a value that arrives once, and both
    /// `resolve` and `reject` are handed to the program, which may call either twice.
    fn settle(&mut self, promise: &Rc<RefCell<PromiseState>>, outcome: &Settled) {
        let (reactions, unobserved) = {
            let mut state = promise.borrow_mut();
            if state.settled.is_some() {
                return;
            }
            state.settled = Some(outcome.clone());
            let unobserved = !state.handled && matches!(outcome, Settled::Rejected(_));
            (std::mem::take(&mut state.reactions), unobserved)
        };
        // Recorded now and cleared if a handler attaches later in the same drain, because a
        // handler added on a later tick is still a handler -- `p.catch(...)` on the next line is
        // ordinary code. Keep the promise identity with the reason: handling a different rejected
        // promise must not erase this diagnostic.
        if unobserved && let Settled::Rejected(reason) = &outcome {
            self.realm
                .pending_unhandled
                .borrow_mut()
                .push((Rc::clone(promise), reason.clone()));
        }
        for reaction in reactions {
            self.realm
                .microtasks
                .borrow_mut()
                .push_back(Job::React(outcome.clone(), reaction));
        }
    }

    /// Resolve a promise with a value, adopting it when that value is itself a promise.
    ///
    /// Adoption is what makes `then` chainable: returning a promise from a handler waits for it
    /// rather than fulfilling with the promise object. It needs no special machinery -- waiting
    /// for `inner` and settling `promise` with whatever it produces is exactly a reaction with no
    /// handlers, which passes both outcomes through unchanged.
    fn resolve_promise(&mut self, promise: &Rc<RefCell<PromiseState>>, value: Value) {
        let Value::Promise(inner) = &value else {
            self.settle(promise, &Settled::Fulfilled(value));
            return;
        };
        if Rc::ptr_eq(promise, inner) {
            // Waiting for itself would wait forever, so the specification makes it a TypeError.
            let error = self.construct_error(
                ErrorKind::Type,
                &[Value::String(
                    "Chaining cycle detected for promise".to_owned(),
                )],
            );
            self.settle(promise, &Settled::Rejected(error));
            return;
        }
        self.realm
            .microtasks
            .borrow_mut()
            .push_back(Job::Adopt(Rc::clone(promise), Rc::clone(inner)));
    }

    /// Attach a reaction to a promise, queueing it immediately if the promise has already settled.
    ///
    /// Queued rather than run, even when the outcome is already known: a handler must never run
    /// synchronously from `then`, or the order of a program would depend on *when* a promise
    /// settled rather than on what it does.
    fn react(
        &mut self,
        promise: &Rc<RefCell<PromiseState>>,
        on_fulfilled: Value,
        on_rejected: Value,
        sink: Sink,
        passthrough: bool,
    ) {
        let reaction = Reaction {
            on_fulfilled,
            on_rejected,
            sink,
            passthrough,
        };
        let settled = {
            let mut state = promise.borrow_mut();
            let first = !state.handled;
            state.handled = true;
            if first && matches!(state.settled, Some(Settled::Rejected(_))) {
                // Something is handling this promise after all. Leave unrelated rejected
                // promises pending for the event loop to report.
                let mut pending = self.realm.pending_unhandled.borrow_mut();
                if let Some(index) = pending
                    .iter()
                    .position(|(rejected, _)| Rc::ptr_eq(rejected, promise))
                {
                    pending.remove(index);
                }
            }
            state.settled.clone()
        };
        match settled {
            Some(outcome) => self
                .realm
                .microtasks
                .borrow_mut()
                .push_back(Job::React(outcome, reaction)),
            None => promise.borrow_mut().reactions.push(reaction),
        }
    }

    /// Run one reaction: call its handler, and settle the promise it derives from the result.
    ///
    /// A handler that throws rejects the derived promise rather than failing the program -- which
    /// is the whole reason `call_catching` exists, since a rejection carries the thrown value.
    fn run_reaction(&mut self, outcome: Settled, reaction: Reaction) -> Result<(), ExecutionError> {
        // Neither of these carries handlers: the bookkeeping *is* the reaction.
        match &reaction.sink {
            Sink::Combinator(native, index, state) => {
                return self.combine_step(*native, *index, &Rc::clone(state), outcome);
            }
            Sink::Replay(promise, recorded) => {
                let promise = Rc::clone(promise);
                // A handler that returned a rejected promise replaces the outcome; anything else
                // leaves it as it was, which is what `finally` promises.
                let outcome = match outcome {
                    Settled::Rejected(reason) => Settled::Rejected(reason),
                    Settled::Fulfilled(_) => recorded.clone(),
                };
                self.settle(&promise, &outcome);
                return Ok(());
            }
            Sink::AsyncGenerator(generator, step) => {
                let (generator, step) = (Rc::clone(generator), Rc::clone(step));
                let resume = match outcome {
                    Settled::Fulfilled(value) => Resume::Send(value),
                    Settled::Rejected(reason) => Resume::Throw(reason),
                };
                return self.drive_async_generator(&generator, &step, resume);
            }
            Sink::Async(generator, promise) => {
                let (generator, promise) = (Rc::clone(generator), Rc::clone(promise));
                let resume = match outcome {
                    // The awaited value arrives as the result of the `await`.
                    Settled::Fulfilled(value) => Resume::Send(value),
                    // A rejection is raised at the `await`, so a `try` around it catches it --
                    // which is the entire reason `await` reads like synchronous code.
                    Settled::Rejected(reason) => Resume::Throw(reason),
                };
                return self.drive_async(&generator, &promise, resume);
            }
            Sink::Promise(_) => {}
        }
        let Sink::Promise(derived) = &reaction.sink else {
            unreachable!("checked just above");
        };
        let derived = Rc::clone(derived);

        let (handler, argument) = match &outcome {
            Settled::Fulfilled(value) => (reaction.on_fulfilled.clone(), value.clone()),
            Settled::Rejected(reason) => (reaction.on_rejected.clone(), reason.clone()),
        };
        if handler.is_nullish() {
            // No handler for this outcome, so it passes through unchanged -- which is what makes
            // `.then(f)` propagate a rejection and `.catch(r)` propagate a value.
            match outcome {
                Settled::Fulfilled(value) => self.resolve_promise(&derived, value),
                Settled::Rejected(reason) => {
                    self.settle(&derived, &Settled::Rejected(reason));
                }
            }
            return Ok(());
        }
        // `finally`'s handler takes no argument and its result is discarded, so the only thing
        // it can change is whether the promise rejects -- which is why a throw is still honoured.
        let arguments = if reaction.passthrough {
            Vec::new()
        } else {
            vec![argument]
        };
        match self.call_catching(&handler, Value::Undefined, arguments, self.instruction)? {
            Ok(result) => {
                if reaction.passthrough {
                    // The specification routes a `finally` handler's result through
                    // `Promise.resolve(...).then(...)`, so the outcome it restores arrives two
                    // ticks after a plain `then` queued alongside it. That delay is observable,
                    // so it is reproduced rather than optimised away -- and it is also what lets
                    // a handler returning a promise hold the chain open.
                    let waited = PromiseState::pending();
                    self.resolve_promise(&waited, result);
                    let relay = PromiseState::pending();
                    self.react(
                        &waited,
                        Value::Undefined,
                        Value::Undefined,
                        Sink::Promise(Rc::clone(&relay)),
                        false,
                    );
                    self.react(
                        &relay,
                        Value::Undefined,
                        Value::Undefined,
                        Sink::Replay(derived, outcome),
                        false,
                    );
                } else {
                    self.resolve_promise(&derived, result);
                }
            }
            Err(thrown) => self.settle(&derived, &Settled::Rejected(thrown)),
        }
        Ok(())
    }

    /// `Promise.all`, `allSettled`, `race` and `any` over an already-collected list.
    ///
    /// One implementation because the four differ only in when they settle: `race` and `any` on
    /// the first input to do the right thing, `all` and `allSettled` when every input has. The
    /// tracker object carries the counters, and lives on the heap because each input's reaction
    /// runs later and has to find it.
    fn combine(&mut self, native: Native, items: Vec<Value>) -> Result<Value, ExecutionError> {
        let result = PromiseState::pending();
        let total = items.len();

        if total == 0 {
            // An empty `all` or `allSettled` is already done; an empty `any` has no input that
            // could succeed, so it rejects, and an empty `race` never settles at all.
            match native {
                Native::PromiseAll | Native::PromiseAllSettled => {
                    let empty = Value::Array(ArrayData::new(Vec::new()));
                    self.settle(&result, &Settled::Fulfilled(empty));
                }
                Native::PromiseAny => {
                    let error = self.aggregate_error(Vec::new());
                    self.settle(&result, &Settled::Rejected(error));
                }
                _ => {}
            }
            return Ok(Value::Promise(result));
        }

        let state = Rc::new(Combinator {
            slots: RefCell::new(vec![Value::Undefined; total]),
            outstanding: std::cell::Cell::new(total),
            result: Rc::clone(&result),
        });
        for (index, item) in items.into_iter().enumerate() {
            // Every input goes through the same resolution a `Promise.resolve` would, so a plain
            // value participates on the same terms as a promise -- one tick later, like
            // everything else.
            let input = match item {
                Value::Promise(promise) => promise,
                other => {
                    let promise = PromiseState::pending();
                    self.resolve_promise(&promise, other);
                    promise
                }
            };
            self.react(
                &input,
                Value::Undefined,
                Value::Undefined,
                Sink::Combinator(native, index, Rc::clone(&state)),
                false,
            );
        }
        Ok(Value::Promise(result))
    }

    /// One input of a combinator settling.
    ///
    /// The four rules in one place, because they differ only in which outcome ends the whole
    /// thing and what it collects:
    ///
    /// - `all` fails on the first rejection and collects fulfilments.
    /// - `any` succeeds on the first fulfilment and collects rejections.
    /// - `race` takes the first outcome of either kind.
    /// - `allSettled` never fails and collects a descriptor for each.
    fn combine_step(
        &mut self,
        native: Native,
        index: usize,
        state: &Rc<Combinator>,
        outcome: Settled,
    ) -> Result<(), ExecutionError> {
        // The first input to decide the answer settles the result, and `settle` ignores the rest
        // -- so there is no separate "already done" flag to keep in step with it.
        match (native, &outcome) {
            (Native::PromiseRace, _) => {
                self.settle(&state.result, &outcome);
                return Ok(());
            }
            (Native::PromiseAll, Settled::Rejected(_))
            | (Native::PromiseAny, Settled::Fulfilled(_)) => {
                self.settle(&state.result, &outcome);
                return Ok(());
            }
            _ => {}
        }

        let collected = match (native, &outcome) {
            (Native::PromiseAllSettled, Settled::Fulfilled(value)) => {
                let mut data = ObjectData::default();
                data.set("status", Value::String("fulfilled".to_owned()));
                data.set("value", value.clone());
                Value::Object(Rc::new(RefCell::new(data)))
            }
            (Native::PromiseAllSettled, Settled::Rejected(reason)) => {
                let mut data = ObjectData::default();
                data.set("status", Value::String("rejected".to_owned()));
                data.set("reason", reason.clone());
                Value::Object(Rc::new(RefCell::new(data)))
            }
            (_, Settled::Fulfilled(value)) => value.clone(),
            (_, Settled::Rejected(reason)) => reason.clone(),
        };
        if let Some(slot) = state.slots.borrow_mut().get_mut(index) {
            *slot = collected;
        }

        state
            .outstanding
            .set(state.outstanding.get().saturating_sub(1));
        if state.outstanding.get() > 0 {
            return Ok(());
        }
        let gathered = Value::Array(ArrayData::new(state.slots.borrow().clone()));
        match native {
            // Every input rejected, and `any` reports them together rather than picking one.
            Native::PromiseAny => {
                let error = self.aggregate_error(state.slots.borrow().clone());
                self.settle(&state.result, &Settled::Rejected(error));
            }
            _ => self.settle(&state.result, &Settled::Fulfilled(gathered)),
        }
        Ok(())
    }

    /// What `Promise.any` rejects with when nothing succeeded: the reasons, and a sentence.
    ///
    /// `errors` is not enumerated, for the same reason an error's `message` is not -- it is the
    /// error saying what happened rather than a property of an object -- and an empty `any`
    /// carries an empty one rather than none, so a handler can read `error.errors.length`
    /// without knowing how the list it was given was empty.
    fn aggregate_error(&mut self, reasons: Vec<Value>) -> Value {
        let error = self.construct_error(
            ErrorKind::Error,
            &[Value::String("All promises were rejected".to_owned())],
        );
        if let Value::Object(object) = &error {
            object
                .borrow_mut()
                .set_non_enumerable("errors", Value::Array(ArrayData::new(reasons)));
        }
        error
    }

    /// Run every queued job, moving the virtual clock forward to reach each timer.
    ///
    /// This is the event loop. Microtasks drain completely before any timer runs and again after
    /// each one, which is the ordering the language specifies and the reason a promise settles
    /// before a `setTimeout(f, 0)` queued earlier.
    ///
    /// Time only moves when a timer is due, so a program that schedules an hour out finishes
    /// instantly and always in the same order. Nothing here reads the host clock.
    fn run_event_loop(&mut self) -> Result<(), ExecutionError> {
        loop {
            self.drain_microtasks()?;
            // Checked between turns, not at each settlement: `Promise.reject(x)` followed by a
            // `.catch` on the next line must not be a failure, and by the time the queue is
            // empty every handler that was going to attach has.
            self.check_unhandled_rejection()?;
            let Some(index) = self.next_due_timer() else {
                return Ok(());
            };
            // Rescheduled *before* running, and under the same id, so a `clearInterval` inside
            // the callback cancels the next run rather than a run that no longer exists.
            let (callback, arguments, repeat) = {
                let timers = self.realm.timers.borrow();
                let timer = &timers[index];
                let now = *self.realm.now.borrow();
                *self.realm.now.borrow_mut() = now.max(timer.due);
                (
                    timer.callback.clone(),
                    timer.arguments.clone(),
                    timer.repeat,
                )
            };
            match repeat {
                Some(delay) => {
                    let sequence = {
                        let mut next = self.realm.next_sequence.borrow_mut();
                        *next += 1;
                        *next
                    };
                    let now = *self.realm.now.borrow();
                    let mut timers = self.realm.timers.borrow_mut();
                    let timer = &mut timers[index];
                    timer.due = now + delay.max(1.0);
                    timer.sequence = sequence;
                }
                None => {
                    self.realm.timers.borrow_mut().remove(index);
                }
            }
            self.call_value(&callback, Value::Undefined, arguments, self.instruction)?;
        }
    }

    /// Report a rejection nothing ever handled.
    ///
    /// A promise that rejects with nothing attached is a silent failure: the program looks like
    /// it worked. Node prints a warning and exits non-zero; this fails the run, which is the same
    /// judgement stated more plainly and matches how an uncaught throw is treated.
    fn check_unhandled_rejection(&mut self) -> Result<(), ExecutionError> {
        let Some((_, reason)) = ({
            let mut pending = self.realm.pending_unhandled.borrow_mut();
            (!pending.is_empty()).then(|| pending.remove(0))
        }) else {
            return Ok(());
        };
        let trace = captured_trace(&reason);
        let instruction = self.instruction;
        let described = self.describe_uncaught(&reason, instruction);
        Err(ExecutionError::Uncaught {
            instruction,
            message: format!("(in promise) {described}"),
            trace,
        })
    }

    /// Run microtasks until none are left, including ones queued by the ones already running.
    fn drain_microtasks(&mut self) -> Result<(), ExecutionError> {
        while let Some(job) = { self.realm.microtasks.borrow_mut().pop_front() } {
            match job {
                Job::Callback(callback, arguments) => {
                    self.call_value(&callback, Value::Undefined, arguments, self.instruction)?;
                }
                Job::React(outcome, reaction) => self.run_reaction(outcome, reaction)?,
                Job::Adopt(outer, inner) => self.react(
                    &inner,
                    Value::Undefined,
                    Value::Undefined,
                    Sink::Promise(outer),
                    false,
                ),
            }
        }
        Ok(())
    }

    /// The timer to run next: the earliest due, and among those the one scheduled first.
    fn next_due_timer(&self) -> Option<usize> {
        self.realm
            .timers
            .borrow()
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                left.due
                    .total_cmp(&right.due)
                    .then(left.sequence.cmp(&right.sequence))
            })
            .map(|(index, _)| index)
    }

    /// Take the result of a step, turning an error a program could catch into a thrown value.
    ///
    /// `Ok(None)` means control has already moved to a handler and the caller should go round the
    /// loop again rather than using a result that does not exist.
    fn catchable<T>(
        &mut self,
        result: Result<T, ExecutionError>,
        instruction: usize,
    ) -> Result<Option<T>, ExecutionError> {
        match result {
            Ok(value) => Ok(Some(value)),
            // The Rust stack is flat again here, so the handler that could not be reached
            // from inside a callback can take the value now.
            Err(ExecutionError::ThrownAcrossNative) => {
                let thrown = self.caught.take().unwrap_or(Value::Undefined);
                self.unwind(thrown, instruction)?;
                Ok(None)
            }
            Err(error) => match error.as_javascript_error() {
                Some((kind, message)) => {
                    self.throw_error(kind, message, instruction)?;
                    Ok(None)
                }
                None => Err(error),
            },
        }
    }

    /// Raise a JavaScript error of `kind` from the VM itself.
    ///
    /// The point of building a real error object rather than failing outright is that a program
    /// can catch it: `try { null.x } catch (e) { e instanceof TypeError }` is ordinary code, and
    /// a fatal execution error would make it unwritable.
    /// The receiver an array method should walk, when one is being redirected onto something
    /// that is not an array.
    ///
    /// `Array.prototype.slice.call(arguments)` and `[].forEach.call(nodeList, ...)` are why the
    /// array methods are generic, and redirection is the only way one of them can meet a
    /// non-array receiver: reading `slice` off a plain object finds nothing, so `obj.slice()` is
    /// a `TypeError` here exactly as it is in node. A string walks as its characters; anything
    /// with a `length` walks as its indexed properties. The copy is not written back, so a
    /// mutating method redirected this way changes nothing -- which is visible in the untouched
    /// object rather than hidden in a wrong result.
    fn as_array_like(
        &mut self,
        callee: &Value,
        receiver: Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let walks_an_array = match callee {
            Value::Callback(method) => !matches!(
                method,
                CallbackMethod::GeneratorNext | CallbackMethod::GeneratorReturn
            ),
            Value::Native(native) => native.walks_an_array(),
            _ => false,
        };
        if !walks_an_array || matches!(receiver, Value::Array(_)) {
            return Ok(receiver);
        }
        let items = match &receiver {
            // A typed array already *is* the list; reading it through `length` and indices would
            // work too, and this skips the round trip.
            Value::TypedArray(view) => view.items(),
            Value::String(text) => text
                .chars()
                .map(|character| Value::String(character.to_string()))
                .collect(),
            Value::Object(_) | Value::Function(_) => {
                let length = self
                    .read_property(&receiver, "length", instruction)?
                    .as_number();
                if length.is_finite() && length > 0.0 {
                    let mut items = Vec::with_capacity(length as usize);
                    for index in 0..length as usize {
                        items.push(self.read_property(
                            &receiver,
                            index.to_string(),
                            instruction,
                        )?);
                    }
                    items
                } else {
                    Vec::new()
                }
            }
            // A number, a boolean, `null`: none has indexed properties, so the walk is empty --
            // which is what `Array.prototype.map.call(1, f)` gives.
            _ => Vec::new(),
        };
        Ok(Value::Array(ArrayData::new(items)))
    }

    /// The realm's global object, created on first use.
    fn global_object(&mut self) -> Value {
        if let Some(existing) = &*self.realm.global_object.borrow() {
            return existing.clone();
        }
        let mut globals = ObjectData::default();
        for kind in ErrorKind::ALL {
            globals.set(kind.name(), Value::Native(Native::Error(kind)));
        }
        globals.set("Array", Value::Native(Native::ArrayConstructor));
        globals.set("Object", Value::Native(Native::ObjectConstructor));
        globals.set("JSON", Value::Native(Native::Json));
        globals.set("Symbol", Value::Native(Native::SymbolConstructor));
        globals.set("String", Value::Native(Native::StringConstructor));
        globals.set("Math", Value::Native(Native::Math));
        globals.set("Promise", Value::Native(Native::PromiseConstructor));
        globals.set("RegExp", Value::Native(Native::RegExpConstructor));
        globals.set("Date", Value::Native(Native::DateConstructor));
        globals.set("WeakMap", Value::Native(Native::WeakMapConstructor));
        globals.set("WeakSet", Value::Native(Native::WeakSetConstructor));
        globals.set("Set", Value::Native(Native::SetConstructor));
        globals.set("Map", Value::Native(Native::MapConstructor));
        globals.set("performance", Value::Native(Native::Performance));
        globals.set("Reflect", Value::Native(Native::Reflect));
        globals.set("BigInt", Value::Native(Native::BigIntConstructor));
        globals.set("Number", Value::Native(Native::NumberConstructor));
        globals.set("Boolean", Value::Native(Native::BooleanConstructor));
        globals.set("Function", Value::Native(Native::FunctionConstructor));
        globals.set("escape", Value::Native(Native::Escape));
        globals.set("unescape", Value::Native(Native::Unescape));
        globals.set("encodeURI", Value::Native(Native::EncodeUri));
        globals.set(
            "encodeURIComponent",
            Value::Native(Native::EncodeUriComponent),
        );
        globals.set("decodeURI", Value::Native(Native::DecodeUri));
        globals.set(
            "decodeURIComponent",
            Value::Native(Native::DecodeUriComponent),
        );
        globals.set("Proxy", Value::Native(Native::ProxyConstructor));
        globals.set("ArrayBuffer", Value::Native(Native::ArrayBufferConstructor));
        globals.set(
            "Int8Array",
            Value::Native(Native::TypedArrayConstructor(TypedKind::Int8)),
        );
        globals.set(
            "Uint8Array",
            Value::Native(Native::TypedArrayConstructor(TypedKind::Uint8)),
        );
        globals.set(
            "Uint8ClampedArray",
            Value::Native(Native::TypedArrayConstructor(TypedKind::Uint8Clamped)),
        );
        globals.set(
            "Int16Array",
            Value::Native(Native::TypedArrayConstructor(TypedKind::Int16)),
        );
        globals.set(
            "Uint16Array",
            Value::Native(Native::TypedArrayConstructor(TypedKind::Uint16)),
        );
        globals.set(
            "Int32Array",
            Value::Native(Native::TypedArrayConstructor(TypedKind::Int32)),
        );
        globals.set(
            "Uint32Array",
            Value::Native(Native::TypedArrayConstructor(TypedKind::Uint32)),
        );
        globals.set(
            "Float32Array",
            Value::Native(Native::TypedArrayConstructor(TypedKind::Float32)),
        );
        globals.set(
            "Float64Array",
            Value::Native(Native::TypedArrayConstructor(TypedKind::Float64)),
        );

        globals.set("NaN", Value::Number(f64::NAN));
        globals.set("Infinity", Value::Number(f64::INFINITY));
        let object = Rc::new(RefCell::new(globals));
        let value = Value::Object(Rc::clone(&object));
        // The global object holds itself under both of its names, which is how a program reaches
        // it without a keyword: `global.Object === Object` is the standard test for "am I in a
        // realm that has one".
        object.borrow_mut().set("globalThis", value.clone());
        object.borrow_mut().set("global", value.clone());
        *self.realm.global_object.borrow_mut() = Some(value.clone());
        value
    }

    /// The realm's `process`, created on first use.
    fn process_object(&mut self) -> Value {
        if let Some(existing) = &*self.realm.process_object.borrow() {
            return existing.clone();
        }
        let mut process = ObjectData::default();
        // Empty and writable: a program that sets `process.env.X` finds it again, and one that
        // reads an unset flag gets undefined, which is what a production build wants.
        process.set(
            "env",
            Value::Object(Rc::new(RefCell::new(ObjectData::default()))),
        );
        process.set("argv", Value::Array(ArrayData::new(Vec::new())));
        // Not "darwin" or "linux": this realm is not either of them, and claiming to be one
        // invites a program to take a path nothing here implements.
        process.set("platform", Value::String("kiln".to_owned()));
        process.set("version", Value::String("kiln".to_owned()));
        let value = Value::Object(Rc::new(RefCell::new(process)));
        *self.realm.process_object.borrow_mut() = Some(value.clone());
        value
    }

    /// What `arguments` refers to in the running function.
    ///
    /// An arrow has none of its own and sees the enclosing function's, which it captured when it
    /// was created. Everything else sees the call it is in.
    fn visible_arguments(&self) -> Vec<Value> {
        match self
            .function
            .as_ref()
            .and_then(|closure| closure.captured_arguments.as_ref())
        {
            Some(captured) => captured.clone(),
            None => self.arguments.clone(),
        }
    }

    /// The call stack as names, innermost first, at the moment this error object was built.
    ///
    /// A closure that was never given a name shows as `<anonymous>` -- which is most of them in
    /// a bundle, but the named ones between them are usually enough to locate the call.
    fn trace(&self) -> Vec<String> {
        std::iter::once(self.function.as_ref())
            .chain(
                self.frames
                    .iter()
                    .rev()
                    .map(|frame| frame.function.as_ref()),
            )
            .map(|function| match function {
                Some(closure) => closure.name.as_deref().unwrap_or("<anonymous>").to_owned(),
                None => "<module>".to_owned(),
            })
            .collect()
    }

    fn throw_error(
        &mut self,
        kind: ErrorKind,
        message: String,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let error = self.construct_error(kind, &[Value::String(message)]);
        self.unwind(error, instruction)
    }

    /// `new Set(iterable)` or `new Map(entries)`.
    fn construct_collection(
        &mut self,
        kind: Native,
        arguments: &[Value],
        current: usize,
    ) -> Result<Value, ExecutionError> {
        let is_map = matches!(kind, Native::MapConstructor | Native::WeakMapConstructor);
        let weak = matches!(
            kind,
            Native::WeakMapConstructor | Native::WeakSetConstructor
        );
        let mut data = ObjectData::default();
        data.set(
            internal_key("entries"),
            Value::Array(ArrayData::new(Vec::new())),
        );
        if is_map {
            data.set(internal_key("map"), Value::Boolean(true));
        }
        if weak {
            data.set(internal_key("weak"), Value::Boolean(true));
        }
        data.set("size", Value::Number(0.0));
        let collection = Value::Object(Rc::new(RefCell::new(data)));
        // A seeded collection goes through `add`/`set`, so deduplication and size accounting are
        // the same code the explicit calls use rather than a second copy of it.
        if let Some(source) = arguments.first().cloned()
            && !matches!(source, Value::Undefined | Value::Null)
        {
            for item in self.drain_iterable(&source, current)? {
                let (key, value) = if is_map {
                    (
                        self.computed(&item, &Value::Number(0.0)),
                        self.computed(&item, &Value::Number(1.0)),
                    )
                } else {
                    (item, Value::Undefined)
                };
                call_native(
                    if is_map {
                        Native::MapSet
                    } else {
                        Native::SetAdd
                    },
                    &collection,
                    &[key, value],
                )?;
            }
        }
        Ok(collection)
    }

    /// Build the object a native parent constructor produces, for `super()` in a derived class.
    ///
    /// `None` for a parent that has no object of its own to make -- the derived class then keeps
    /// the instance it allocated, which is the ordinary case.
    fn construct_exotic(
        &mut self,
        parent: Native,
        arguments: &[Value],
        current: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        Ok(Some(match parent {
            Native::Error(kind) => self.construct_error(kind, arguments),
            Native::ArrayConstructor => construct_array(arguments),
            Native::SetConstructor
            | Native::MapConstructor
            | Native::WeakSetConstructor
            | Native::WeakMapConstructor => {
                self.construct_collection(parent, arguments, current)?
            }
            Native::DateConstructor | Native::RegExpConstructor => {
                self.dispatch_native(parent, &Value::Undefined, arguments, current)?
            }
            _ => return Ok(None),
        }))
    }

    /// `new callee(...arguments)`.
    ///
    /// Leaves the instance on the stack for a native constructor. For a user function it pushes a
    /// frame instead, and the instance arrives when that frame returns -- which is why this
    /// cannot simply hand one back.
    fn construct(
        &mut self,
        callee: Value,
        arguments: Vec<Value>,
        current: usize,
    ) -> Result<(), ExecutionError> {
        match callee {
            // `new Proxy(target, handler)` is the constructor; a proxy *of* something
            // constructible is the `construct` trap.
            Value::Native(Native::ProxyConstructor) => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let handler = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                self.stack.push(Value::Proxy(Rc::new(ProxyData {
                    target,
                    handler,
                    revoked: std::cell::Cell::new(false),
                })));
            }
            Value::Proxy(proxy) => {
                let inner = proxy.target.clone();
                match self.proxy_trap(&proxy, "construct", current)? {
                    Some(trap) => {
                        let handler = proxy.handler.clone();
                        let list = Value::Array(ArrayData::new(arguments));
                        let built = self.call_value(
                            &trap,
                            handler,
                            vec![inner, list, Value::Proxy(Rc::clone(&proxy))],
                            current,
                        )?;
                        self.stack.push(built);
                    }
                    None => return self.construct(inner, arguments, current),
                }
            }
            Value::Native(Native::Error(kind)) => {
                let error = self.construct_error(kind, &arguments);
                self.stack.push(error);
            }
            Value::Native(Native::ArrayConstructor) => {
                let array = construct_array(&arguments);
                self.stack.push(array);
            }
            // `new Object(v)` and `Object(v)` are the same operation: the specification routes
            // both to ToObject, and differs only in a `NewTarget` this interpreter has no other
            // use for.  Sharing the call behaviour keeps them from drifting apart.
            Value::Native(Native::ObjectConstructor) => {
                let object = call_native(Native::ObjectConstructor, &Value::Undefined, &arguments)?;
                self.stack.push(object);
            }
            Value::Native(
                native @ (Native::ArrayBufferConstructor | Native::TypedArrayConstructor(_)),
            ) => {
                let built = call_native(native, &Value::Undefined, &arguments)?;
                self.stack.push(built);
            }
            Value::Native(Native::DateConstructor) => {
                let date = self.dispatch_native(
                    Native::DateConstructor,
                    &Value::Undefined,
                    &arguments,
                    current,
                )?;
                self.stack.push(date);
            }
            Value::Native(Native::RegExpConstructor) => {
                let regexp = self.dispatch_native(
                    Native::RegExpConstructor,
                    &Value::Undefined,
                    &arguments,
                    current,
                )?;
                self.stack.push(regexp);
            }
            Value::Native(Native::PromiseConstructor) => {
                let promise = PromiseState::pending();
                let executor = arguments.first().cloned().unwrap_or(Value::Undefined);
                // The executor runs *now*, synchronously, which is what lets it capture
                // something and settle the promise later.
                let outcome = self.call_catching(
                    &executor,
                    Value::Undefined,
                    vec![
                        Value::Settler(Rc::clone(&promise), false),
                        Value::Settler(Rc::clone(&promise), true),
                    ],
                    current,
                )?;
                // An executor that throws rejects the promise, rather than failing the program:
                // `new Promise(() => { throw e })` is how a rejection is often written.
                if let Err(thrown) = outcome {
                    self.settle(&promise, &Settled::Rejected(thrown));
                }
                self.stack.push(Value::Promise(promise));
            }
            Value::Native(
                kind @ (Native::SetConstructor
                | Native::MapConstructor
                | Native::WeakSetConstructor
                | Native::WeakMapConstructor),
            ) => {
                let collection = self.construct_collection(kind, &arguments, current)?;
                self.stack.push(collection);
            }
            Value::Function(closure) => {
                // `new` creates the instance, runs the body with it as `this`, and
                // yields the instance -- unless the body returned an object, which
                // wins. That override is what makes factory constructors work.
                let instance = self.instantiate(&closure);
                self.stack.push(Value::Undefined);
                let argc = arguments.len();
                for argument in arguments {
                    self.stack.push(argument);
                }
                self.call_with_this(
                    Rc::clone(&closure),
                    argc as u16,
                    current,
                    instance.clone(),
                    Some(instance),
                )?;
            }
            other => {
                return Err(ExecutionError::NotConstructable {
                    instruction: current,
                    value: other.type_name(),
                });
            }
        }
        Ok(())
    }

    /// Build an error instance whose prototype is this run's prototype for that kind.
    fn construct_error(&mut self, kind: ErrorKind, arguments: &[Value]) -> Value {
        let prototype = self.error_prototype(kind);
        let error = build_error(kind, arguments);
        if let Value::Object(object) = &error {
            let mut object = object.borrow_mut();
            object.set_prototype(prototype);
            // Recorded where the error was *made*, not where it was reported: a rejection
            // surfaces after the stack that produced it is gone, and that is exactly the case
            // where a trace is worth having.
            object.set(
                internal_key("trace"),
                Value::Array(ArrayData::new(
                    self.trace().into_iter().map(Value::String).collect(),
                )),
            );
        }
        error
    }

    /// The object a `new` call starts from: an empty object whose prototype is the function's.
    ///
    /// The prototype object is created on first use rather than with every closure, so an
    /// ordinary function never used as a constructor never allocates one.
    fn instantiate(&mut self, closure: &Rc<Closure>) -> Value {
        let prototype = prototype_object(closure);
        let mut instance = ObjectData::default();
        instance.set_prototype(prototype);
        Value::Object(Rc::new(RefCell::new(instance)))
    }

    /// Build a closure, resolving each upvalue descriptor against the frame creating it.
    fn make_closure(&mut self, function: u32) -> Result<Rc<Closure>, ExecutionError> {
        let Some(proto) = self.program.functions.get(function as usize) else {
            return Err(ExecutionError::UnknownFunction { function });
        };
        let mut upvalues = Vec::with_capacity(proto.upvalues.len());
        for descriptor in &proto.upvalues {
            let cell = match descriptor {
                Upvalue::ParentLocal(slot) => self.cell(*slot as usize)?,
                Upvalue::ParentUpvalue(index) => {
                    let Some(current) = &self.function else {
                        // Only a nested function has a parent closure to capture from, and the
                        // compiler never emits ParentUpvalue at module level.
                        return Err(ExecutionError::MissingUpvalue { index: *index });
                    };
                    current
                        .upvalues
                        .get(*index as usize)
                        .cloned()
                        .ok_or(ExecutionError::MissingUpvalue { index: *index })?
                }
            };
            upvalues.push(cell);
        }
        Ok(Rc::new(Closure {
            function,
            name: proto.name.as_deref().map(Rc::from),
            upvalues,
            // An arrow takes `this` from where it is written; an ordinary function takes it from
            // how it is called, so it captures nothing here.
            captured_this: if proto.arrow {
                Some(self.this_value.clone())
            } else {
                None
            },
            program: Rc::clone(&self.program),
            captured_new_target: proto.arrow.then(|| self.new_target.clone()),
            captured_arguments: if proto.arrow {
                // What `arguments` means *here*, which for an arrow defined inside another
                // arrow is what that one captured -- not its own call's list, which is its rest
                // parameter's business and nothing else's.
                Some(self.visible_arguments())
            } else {
                None
            },
            prototype: RefCell::new(None),
            properties: RefCell::new(ObjectData::default()),
        }))
    }

    /// A property read. Missing properties are `undefined`, which is not an error in JavaScript.
    ///
    /// Reads off a primitive currently yield `undefined` rather than consulting a prototype:
    /// `"abc".length` needs `String.prototype`, which does not exist yet. Returning undefined is
    /// wrong for those, and it is wrong in the direction that shows up as a visibly missing
    /// value rather than a plausible one.
    /// `replace` and `replaceAll` where the pattern is a regexp or the replacement is a function.
    ///
    /// Both of those need the machine: one to run the engine, the other to call back into the
    /// program. The plain string-for-string form stays with the pure built-ins.
    fn replace_with_pattern(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        native: Native,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::String(text) = receiver else {
            return Ok(Value::Undefined);
        };
        let subject: Vec<u16> = text.encode_utf16().collect();
        let replacement = arguments.get(1).cloned().unwrap_or(Value::Undefined);
        let callable = matches!(
            replacement,
            Value::Function(_) | Value::Native(_) | Value::Callback(_)
        );

        // A string pattern replaces the first occurrence, or every one for `replaceAll`; a
        // regexp replaces every one only when it carries `g`.
        let (matches, from_regexp) = match arguments.first() {
            Some(Value::RegExp(regexp)) => {
                let all = regexp.global;
                let found = if all {
                    regexp_search_all(regexp, &subject)
                } else {
                    regexp_search(regexp, &subject, 0).into_iter().collect()
                };
                (found, true)
            }
            other => {
                let needle: Vec<u16> = other
                    .map_or_else(String::new, Value::to_display_string)
                    .encode_utf16()
                    .collect();
                (
                    literal_matches(
                        &subject,
                        &needle,
                        matches!(native, Native::StringReplaceAll),
                    ),
                    false,
                )
            }
        };
        let _ = from_regexp;

        let mut out = String::new();
        let mut copied = 0;
        for (range, groups, named) in matches {
            out.push_str(&utf16_slice(&subject, copied..range.start));
            let matched = utf16_slice(&subject, range.clone());
            if callable {
                // The handler receives the match, then each group, then where it was found and
                // the whole subject -- the same arguments the specification passes.
                let mut call_arguments = vec![Value::String(matched.clone())];
                for group in groups.iter().skip(1) {
                    call_arguments.push(match group {
                        Some(range) => Value::String(utf16_slice(&subject, range.clone())),
                        None => Value::Undefined,
                    });
                }
                call_arguments.push(Value::Number(range.start as f64));
                call_arguments.push(Value::String(text.clone()));
                if !named.is_empty() {
                    call_arguments.push(named_groups_object(&subject, &named));
                }
                let produced =
                    self.call_value(&replacement, Value::Undefined, call_arguments, instruction)?;
                out.push_str(&produced.to_display_string());
            } else {
                out.push_str(&expand_replacement(
                    &replacement.to_display_string(),
                    &subject,
                    &range,
                    &groups,
                    &named,
                ));
            }
            copied = range.end;
        }
        out.push_str(&utf16_slice(&subject, copied..subject.len()));
        Ok(Value::String(out))
    }

    /// The pattern argument of a string method, promoted to a regexp when it is a plain string.
    ///
    /// `"a.b".split(".")` splits on a literal dot while `"a.b".split(/./)` splits on anything, so
    /// a string argument is escaped rather than compiled as a pattern.
    fn pattern_argument(
        &mut self,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        match arguments.first() {
            Some(value @ Value::RegExp(_)) => Ok(Some(value.clone())),
            None | Some(Value::Undefined) => Ok(None),
            Some(value) => {
                let escaped = regress::escape(&value.to_display_string());
                match build_regexp(&escaped, "g") {
                    Some(regexp) => Ok(Some(regexp)),
                    None => {
                        self.throw_error(
                            ErrorKind::Syntax,
                            "Invalid regular expression".to_owned(),
                            instruction,
                        )?;
                        Ok(None)
                    }
                }
            }
        }
    }

    /// `ToPrimitive`: reduce an object to a value the operators can work on.
    ///
    /// Named `as_primitive` rather than `to_primitive` because it takes `&mut self`: reducing an
    /// object *runs* one of its methods, which is the whole reason it lives here.
    ///
    /// An object becomes a primitive by *calling* one of its own methods, which is why this is on
    /// the machine and why `"" + obj` cannot be decided by looking at the value alone. `valueOf`
    /// is tried first unless a string is wanted, which is the order the specification gives and
    /// the reason `new Date() - 0` is a number while `` `${date}` `` is a sentence.
    ///
    /// A method that hands back another object is skipped rather than used: the point is to reach
    /// a primitive, and one that does not is no closer.
    fn as_primitive(
        &mut self,
        value: &Value,
        prefer_string: bool,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if !matches!(
            value,
            Value::Object(_) | Value::Function(_) | Value::Array(_)
        ) {
            return Ok(value.clone());
        }
        let order = if prefer_string {
            ["toString", "valueOf"]
        } else {
            ["valueOf", "toString"]
        };
        for name in order {
            let method = self.property(value, name);
            if !matches!(
                method,
                Value::Function(_) | Value::Native(_) | Value::Callback(_)
            ) {
                continue;
            }
            let result = self.call_value(&method, value.clone(), Vec::new(), instruction)?;
            if !matches!(
                result,
                Value::Object(_) | Value::Function(_) | Value::Array(_)
            ) {
                return Ok(result);
            }
        }
        // Nothing usable, so the built-in conversion stands.
        Ok(Value::String(value.to_display_string()))
    }

    /// Describe a value that is about to end the run as an uncaught throw.
    ///
    /// A host reports an uncaught value the way the program would print it, which for a
    /// user-defined error type means its own `toString`. Test262's whole assertion library throws
    /// `Test262Error`, whose `toString` is an ordinary function, so without this every failure in
    /// the suite reads `uncaught [object Object]` and the assertion message -- the only part that
    /// says what broke -- is lost.
    ///
    /// This runs user code, which is only acceptable because both callers are terminal: the run
    /// is ending either way. A `toString` that throws, or that answers something other than a
    /// string, falls back to the built-in description rather than replacing one uncaught error
    /// with another.
    fn describe_uncaught(&mut self, thrown: &Value, instruction: usize) -> String {
        if has_error_to_string(thrown) {
            return error_to_string(thrown);
        }
        if matches!(
            thrown,
            Value::Object(_) | Value::Function(_) | Value::Array(_)
        ) && let Ok(Value::String(text)) = self.as_primitive(thrown, true, instruction)
        {
            return text;
        }
        describe_thrown(thrown)
    }

    /// Read a property, calling a getter when the slot holds one.
    ///
    /// Separate from [`Vm::property`], which answers with the slot's contents, because reading an
    /// accessor is a *call* and only the machine can make one. Every program-visible read goes
    /// through this; the VM's own reads of reserved slots do not need to.
    /// One property of a proxy described, through its trap or through its target.
    fn proxy_descriptor(
        &mut self,
        target: &Value,
        key: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Proxy(proxy) = target else {
            return self.dispatch_native(
                Native::ObjectGetOwnPropertyDescriptor,
                &Value::Undefined,
                &[target.clone(), key.clone()],
                instruction,
            );
        };
        let proxy = Rc::clone(proxy);
        let inner = proxy.target.clone();
        match self.proxy_trap(&proxy, "getOwnPropertyDescriptor", instruction)? {
            Some(trap) => {
                let handler = proxy.handler.clone();
                let list = vec![inner, Self::trap_key(key)];
                self.call_value(&trap, handler, list, instruction)
            }
            None => self.proxy_descriptor(&inner, key, instruction),
        }
    }

    /// Run a mutating array method against a proxy, writing the result back through its traps.
    ///
    /// The elements are read out, the method runs on that array, and every element plus `length`
    /// goes back through `set`. Not the specification's element-by-element algorithm -- that one
    /// interleaves reads and writes in an order a handler can observe -- but the same end state,
    /// which is what a copy-on-write handler such as immer's is watching for.
    fn mutate_through_proxy(
        &mut self,
        native: Native,
        target: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let length = self
            .proxy_get(target, &Value::String("length".to_owned()), instruction)?
            .as_number();
        let length = if length.is_finite() {
            length.max(0.0) as usize
        } else {
            0
        };
        let mut items = Vec::with_capacity(length);
        for index in 0..length {
            items.push(self.proxy_get(target, &Value::Number(index as f64), instruction)?);
        }
        let scratch = Value::Array(ArrayData::new(items));
        let answer = self.dispatch_native(native, &scratch, arguments, instruction)?;
        let Value::Array(updated) = &scratch else {
            unreachable!("just built as an array");
        };
        let updated = updated.borrow().clone();
        for (index, item) in updated.iter().enumerate() {
            self.set_computed(
                target,
                &Value::Number(index as f64),
                item.clone(),
                instruction,
            )?;
        }
        // The length last, so a handler watching it sees a consistent array when it fires.
        self.set_computed(
            target,
            &Value::String("length".to_owned()),
            Value::Number(updated.len() as f64),
            instruction,
        )?;
        Ok(answer)
    }

    /// Put the result of a generic array method back into the shape a typed array expects.
    ///
    /// A method that builds a *new* array builds a typed one of the same kind; one that
    /// rearranges in place writes back through the view and answers with it. Everything else --
    /// `join`, `indexOf`, `reduce` -- produces something that is not an array and passes through.
    fn typed_result(
        &self,
        original: &Value,
        produced: Value,
        in_place: bool,
        new_array: bool,
    ) -> Value {
        let Value::TypedArray(view) = original else {
            return produced;
        };
        let Value::Array(items) = &produced else {
            return produced;
        };
        if in_place {
            for (index, item) in items.borrow().iter().enumerate() {
                view.set(index, item.as_number());
            }
            return original.clone();
        }
        if new_array {
            let elements = dense_items(&items.borrow());
            return construct_typed_array(view.kind, &[Value::Array(ArrayData::new(elements))]);
        }
        produced
    }

    /// The key a trap is handed.
    ///
    /// A symbol stays a symbol: a handler that special-cases one -- `if (key === MY_SYMBOL)` --
    /// is the standard way a library hides its own state behind a proxy, and stringifying the
    /// key breaks exactly that. Everything else is a string, as the specification says, so a
    /// numeric index arrives as `"0"`.
    fn trap_key(key: &Value) -> Value {
        match key {
            Value::Symbol(_) => key.clone(),
            other => Value::String(other.property_key()),
        }
    }

    /// The trap `name` names on a proxy's handler, or `None` when the handler does not have one.
    ///
    /// A missing trap means the operation falls through to the target, which is what makes a
    /// handler with one trap a useful thing to write.
    fn proxy_trap(
        &mut self,
        proxy: &Rc<ProxyData>,
        name: &str,
        instruction: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        if proxy.revoked.get() {
            return Err(ExecutionError::RevokedProxy);
        }
        let trap = self.read_property(&proxy.handler, name, instruction)?;
        Ok(is_callable(&trap).then_some(trap))
    }

    fn read_property<K: Into<PropertyKey>>(
        &mut self,
        target: &Value,
        name: K,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let name = name.into();
        if matches!(target, Value::Proxy(_)) {
            return self.proxy_get(target, &key_value(&name), instruction);
        }
        let accessor = match target {
            Value::Object(object) => object.borrow().lookup(name.clone()),
            Value::Function(closure) if name != PropertyKey::String("prototype".to_owned()) => {
                closure.properties.borrow().lookup(name.clone())
            }
            // An array's non-element properties live in an ordinary store with an ordinary
            // chain, so a subclass's accessor is reachable the same way.
            Value::Array(items) => items.properties.borrow().lookup(name.clone()),
            _ => None,
        };
        match accessor {
            Some(Property::Accessor { get, .. }) => match get {
                Some(getter) => self.call_value(&getter, target.clone(), Vec::new(), instruction),
                // Setter-only, so there is nothing to read.
                None => Ok(Value::Undefined),
            },
            _ => Ok(self.property_key(target, &name)),
        }
    }

    /// Route a write through a setter found anywhere on the prototype chain, answering whether
    /// the write was consumed.
    ///
    /// A setter takes the write ahead of creating an own data property, and a getter-only
    /// accessor swallows it. A store that skips this check silently *shadows* the accessor
    /// instead -- which is how `chalk.level = 3` became an ordinary property on the chalk
    /// function and never reached the generator sitting behind the accessor.
    fn inherited_setter(
        &mut self,
        store: &RefCell<ObjectData>,
        target: &Value,
        name: &PropertyKey,
        value: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        // Bound out of the `if let`: the borrow would otherwise still be alive at the setter
        // call, which is ordinary code that may touch this very object.
        let existing = store.borrow().lookup(name.clone());
        let Some(Property::Accessor { set, .. }) = existing else {
            return Ok(false);
        };
        let Some(setter) = set else {
            // Getter-only: the write is discarded in sloppy mode and a TypeError in strict.
            // Modules are strict, but a script is not, and this is shared.
            return Ok(true);
        };
        self.call_value(&setter, target.clone(), vec![value.clone()], instruction)?;
        Ok(true)
    }

    fn property(&self, target: &Value, name: &str) -> Value {
        self.property_key(target, &PropertyKey::String(name.to_owned()))
    }

    fn property_key(&self, target: &Value, key: &PropertyKey) -> Value {
        let Some(name) = string_key(key) else {
            let found = match target {
                Value::Object(object) => object.borrow().get(key.clone()),
                Value::Function(closure) => closure.properties.borrow().get(key.clone()),
                Value::Array(items) => items.properties.borrow().get(key.clone()),
                _ => None,
            };
            if found.is_some() {
                return found.unwrap_or(Value::Undefined);
            }
            if *key == PropertyKey::Symbol(Symbol::WellKnown(WellKnown::Iterator)) {
                return match target {
                    Value::Array(_) => Value::Native(Native::ArrayValues),
                    Value::String(_) => Value::Native(Native::StringValues),
                    Value::Generator(state) if is_async_generator(state) => Value::Undefined,
                    Value::Generator(_) => Value::Native(Native::IteratorSelf),
                    Value::Object(object)
                        if object.borrow().get_own(internal_key("entries")).is_some() =>
                    {
                        let is_map = matches!(
                            object.borrow().get_own(internal_key("map")),
                            Some(&Value::Boolean(true))
                        );
                        Value::Native(if is_map {
                            Native::CollectionEntries
                        } else {
                            Native::CollectionValues
                        })
                    }
                    _ => Value::Undefined,
                };
            }
            if *key == PropertyKey::Symbol(Symbol::WellKnown(WellKnown::AsyncIterator)) {
                return match target {
                    Value::Generator(state) if is_async_generator(state) => {
                        Value::Native(Native::IteratorSelf)
                    }
                    _ => Value::Undefined,
                };
            }
            return Value::Undefined;
        };
        // Every intrinsic kind answers `constructor` with the built-in that makes it. Handled
        // here rather than in each table because the answer is the same question for all of
        // them, and because `new x.constructor(...)` is how a library clones a value whose type
        // it was handed rather than told.
        if name == "constructor" {
            // An own property shadows the virtual constructor supplied by an intrinsic. This is
            // observable on arrays (`a.constructor = C`) and is also how ordinary prototype
            // lookup behaves for every other object.
            let own = match target {
                Value::Array(items) => items.properties.borrow().get(name),
                _ => None,
            };
            if let Some(own) = own {
                return own;
            }
            if let Some(constructor) = intrinsic_constructor(target) {
                return constructor;
            }
        }
        match target {
            // A Set or Map instance is an object whose reserved state marks it. Its methods come
            // from one table shared with `in`, so the two cannot disagree -- and this arm has to
            // precede the ordinary object case, which would otherwise answer `undefined` first.
            Value::Object(object) if object.borrow().get_own(internal_key("entries")).is_some() => {
                let (is_map, weak) = {
                    let object = object.borrow();
                    (
                        matches!(
                            object.get_own(internal_key("map")),
                            Some(&Value::Boolean(true))
                        ),
                        matches!(
                            object.get_own(internal_key("weak")),
                            Some(&Value::Boolean(true))
                        ),
                    )
                };
                // `size` is an ordinary stored property for a strong collection and absent for a
                // weak one, which has no count to report.
                if name == "size" {
                    if weak {
                        return Value::Undefined;
                    }
                    return object.borrow().get(name).unwrap_or(Value::Undefined);
                }
                match collection_property(is_map, weak, name) {
                    Value::Undefined => object.borrow().get(name).unwrap_or(Value::Undefined),
                    method => method,
                }
            }
            Value::Object(object) => {
                let object = object.borrow();
                object.get(name).unwrap_or_else(|| {
                    // An object whose prototype is explicitly `null` inherits nothing --
                    // `Object.create(null)` is how a program asks for a bare dictionary, and
                    // handing it `hasOwnProperty` would defeat the point.
                    if matches!(object.prototype(), Some(Value::Null)) {
                        Value::Undefined
                    } else if name == "constructor" {
                        // The end of the chain is `Object.prototype`, whose `constructor` is
                        // `Object`. Answered here rather than beside the other intrinsics
                        // because a class instance finds its own on the way down, and this must
                        // not shadow it: `x.constructor === Object` is how a library asks
                        // whether something is a *plain* object.
                        Value::Native(Native::ObjectConstructor)
                    } else {
                        inherited_object_property(name)
                    }
                })
            }
            Value::Native(native) => native
                .property(name)
                .unwrap_or_else(|| function_property(name)),
            // `f.prototype` is the object `new f()` gives instances. Reading it creates it, so
            // that `f.prototype.m = ...` before any `new` reaches the same object the instances
            // will see.
            Value::Function(closure) if name != "prototype" => {
                closure
                    .properties
                    .borrow()
                    .get(name)
                    .unwrap_or_else(|| match name {
                        // Neither is stored: both come from the proto, and every closure over one
                        // agrees about them.
                        "name" => {
                            Value::String(closure.name.as_deref().unwrap_or_default().to_owned())
                        }
                        "length" => Value::Number(f64::from(
                            closure
                                .program
                                .functions
                                .get(closure.function as usize)
                                .map_or(0, |proto| proto.parameter_count),
                        )),
                        _ => function_property(name),
                    })
            }
            // A bound function reports the arity still to be supplied and the name of what it
            // wraps, which is what makes a stack trace through one readable.
            Value::Bound(bound) => match name {
                "length" => {
                    let arity = self.property(&bound.target, "length").as_number();
                    Value::Number((arity - bound.arguments.len() as f64).max(0.0))
                }
                "name" => Value::String(format!(
                    "bound {}",
                    self.property(&bound.target, "name").to_display_string()
                )),
                _ => function_property(name),
            },
            // Every other callable answers to the same three, and to nothing else: none of them
            // has a property store to shadow them with.
            Value::Callback(_) | Value::Settler(..) => function_property(name),
            Value::Function(closure) if name == "prototype" => prototype_object(closure),
            // `length` is not an ordinary property of an array: it is derived, and writing it
            // truncates. Reading it is all the corpus needs so far.
            Value::Array(items) if name == "length" => Value::Number(items.borrow().len() as f64),
            // Unbound, so that reading the same method twice yields the same value. The
            // receiver is supplied by `CallMethod`, not captured here.
            // Strings expose `length` and the iterator. `length` is UTF-16 code units, which is
            // what JavaScript counts, and is not the same as either bytes or characters.
            Value::String(text) if name == "length" => {
                Value::Number(text.encode_utf16().count() as f64)
            }
            Value::String(_) => string_property(name),
            Value::Number(_) => number_property(name),
            // The built-in methods first, then the array's own named properties, then whatever
            // a subclass put on its prototype -- which is the order the chain would give if the
            // realm had an `Array.prototype` object to hang the built-ins on.
            Value::Array(items) => items
                .properties
                .borrow()
                .get(name)
                .unwrap_or_else(|| array_property(name)),
            // A regexp's own properties. `lastIndex` is the mutable one, and the reason two
            // evaluations of one literal must not share an object.
            Value::Boolean(_) => match name {
                "toString" | "toLocaleString" => Value::Native(Native::PrimitiveToString),
                "valueOf" => Value::Native(Native::ObjectValueOf),
                _ => Value::Undefined,
            },
            Value::BigInt(_) => match name {
                "toString" | "toLocaleString" => Value::Native(Native::BigIntToString),
                "valueOf" => Value::Native(Native::NumberValueOf),
                _ => Value::Undefined,
            },
            Value::Date(_) => match name {
                "getTime" | "valueOf" => Value::Native(Native::DateField(DateField::Time)),
                // The local readers are the UTC ones: the realm's zone is UTC, which
                // `getTimezoneOffset` reports honestly rather than leaving to be discovered.
                "getFullYear" | "getUTCFullYear" => {
                    Value::Native(Native::DateField(DateField::FullYear))
                }
                "getMonth" | "getUTCMonth" => Value::Native(Native::DateField(DateField::Month)),
                "getDate" | "getUTCDate" => Value::Native(Native::DateField(DateField::Day)),
                "getDay" | "getUTCDay" => Value::Native(Native::DateField(DateField::Weekday)),
                "getHours" | "getUTCHours" => Value::Native(Native::DateField(DateField::Hours)),
                "getMinutes" | "getUTCMinutes" => {
                    Value::Native(Native::DateField(DateField::Minutes))
                }
                "getSeconds" | "getUTCSeconds" => {
                    Value::Native(Native::DateField(DateField::Seconds))
                }
                "getMilliseconds" | "getUTCMilliseconds" => {
                    Value::Native(Native::DateField(DateField::Milliseconds))
                }
                "getTimezoneOffset" => Value::Native(Native::DateField(DateField::TimezoneOffset)),
                // The setters mirror the readers, and for the same reason the local ones are the
                // UTC ones: the realm has one zone (ADR 0015).
                "setTime" => Value::Native(Native::DateSet(DateField::Time)),
                "setFullYear" | "setUTCFullYear" => {
                    Value::Native(Native::DateSet(DateField::FullYear))
                }
                "setMonth" | "setUTCMonth" => Value::Native(Native::DateSet(DateField::Month)),
                "setDate" | "setUTCDate" => Value::Native(Native::DateSet(DateField::Day)),
                "setHours" | "setUTCHours" => Value::Native(Native::DateSet(DateField::Hours)),
                "setMinutes" | "setUTCMinutes" => {
                    Value::Native(Native::DateSet(DateField::Minutes))
                }
                "setSeconds" | "setUTCSeconds" => {
                    Value::Native(Native::DateSet(DateField::Seconds))
                }
                "setMilliseconds" | "setUTCMilliseconds" => {
                    Value::Native(Native::DateSet(DateField::Milliseconds))
                }
                "toISOString" => Value::Native(Native::DateToIso),
                // Not the same function as `toISOString`, and the difference is exactly the
                // case JSON cares about: an instant with no ISO spelling makes `toISOString`
                // throw, where `toJSON` answers null so the enclosing document still serialises.
                "toJSON" => Value::Native(Native::DateToJson),
                "toString" | "toUTCString" | "toLocaleString" | "toLocaleDateString"
                | "toLocaleTimeString" | "toDateString" | "toTimeString" => {
                    Value::Native(Native::DateToString)
                }
                _ => Value::Undefined,
            },
            Value::TypedArray(view) => match name {
                "length" => Value::Number(view.length as f64),
                "byteLength" => Value::Number((view.length * view.kind.width()) as f64),
                "byteOffset" => Value::Number(view.offset as f64),
                "BYTES_PER_ELEMENT" => Value::Number(view.kind.width() as f64),
                "buffer" => Value::ArrayBuffer(Rc::clone(&view.buffer)),
                "set" => Value::Native(Native::TypedArraySet),
                "subarray" => Value::Native(Native::TypedArraySubarray),
                "fill" => Value::Native(Native::TypedArrayFill),
                "constructor" => Value::Native(Native::TypedArrayConstructor(view.kind)),
                // Everything else a typed array answers to is an ordinary array method reading
                // `length` and indices, which the generic-method coercion provides. The ones
                // that *change* an array are excluded: a view has a fixed length, so `push` on
                // one is a TypeError in the specification and would silently push into a copy
                // here.
                other => match array_property(other) {
                    // A view has a fixed length, so the methods that change one are not there:
                    // `push` on a typed array is a TypeError in the specification.
                    Value::Native(native) if native.changes_array_length() => Value::Undefined,
                    method => method,
                },
            },
            Value::ArrayBuffer(bytes) => match name {
                "byteLength" => Value::Number(bytes.borrow().len() as f64),
                "constructor" => Value::Native(Native::ArrayBufferConstructor),
                _ => Value::Undefined,
            },
            Value::RegExp(regexp) => match name {
                "test" => Value::Native(Native::RegExpTest),
                "exec" => Value::Native(Native::RegExpExec),
                // Its own, not the error one: a regexp prints as `/source/flags`, and reusing
                // `Error.prototype.toString` made every regexp stringify as "Error".
                "toString" => Value::Native(Native::RegExpToString),
                "source" => Value::String(regexp.source.clone()),
                "flags" => Value::String(regexp.flags.clone()),
                "global" => Value::Boolean(regexp.global),
                "sticky" => Value::Boolean(regexp.sticky),
                "ignoreCase" => Value::Boolean(regexp.flags.contains('i')),
                "multiline" => Value::Boolean(regexp.flags.contains('m')),
                // Read off the flag string rather than from a field per flag, so a flag nothing
                // else looks at still reads back correctly.
                "dotAll" => Value::Boolean(regexp.flags.contains('s')),
                "unicode" => Value::Boolean(regexp.flags.contains('u')),
                "unicodeSets" => Value::Boolean(regexp.flags.contains('v')),
                "hasIndices" => Value::Boolean(regexp.flags.contains('d')),
                "lastIndex" => Value::Number(regexp.last_index.get() as f64),
                _ => Value::Undefined,
            },
            Value::Promise(_) => match name {
                "then" => Value::Native(Native::PromiseThen),
                "catch" => Value::Native(Native::PromiseCatch),
                "finally" => Value::Native(Native::PromiseFinally),
                _ => Value::Undefined,
            },
            // A generator object is its own iterator, which is what lets one be used directly in
            // `for-of` and in a spread.
            // An async generator's `next` hands back a promise of the step, and it answers to
            // `@@asyncIterator` rather than `@@iterator`: a `for-of` over one would get result
            // objects that are promises, which is exactly the mistake `for await` exists to
            // avoid, so it is left to fail rather than half-work.
            Value::Generator(state) if is_async_generator(state) => match name {
                "next" => Value::Callback(CallbackMethod::AsyncGeneratorNext),
                _ => Value::Undefined,
            },
            // A generator object is its own iterator, which is what lets one be used directly in
            // `for-of` and in a spread.
            Value::Generator(_) => match name {
                "next" => Value::Callback(CallbackMethod::GeneratorNext),
                "return" => Value::Callback(CallbackMethod::GeneratorReturn),
                _ => Value::Undefined,
            },
            _ => Value::Undefined,
        }
    }

    /// Pop `argc` arguments, leaving whatever is beneath them.
    fn take_call_arguments(
        &mut self,
        argc: u16,
        instruction: usize,
    ) -> Result<Vec<Value>, ExecutionError> {
        let argc = argc as usize;
        if self.stack.len() < argc {
            return Err(ExecutionError::StackUnderflow { instruction });
        }
        Ok(self.stack.split_off(self.stack.len() - argc))
    }

    /// Invoke `method` with an explicit receiver.
    ///
    /// Returns `None` when a frame was pushed instead: a compiled function's result arrives via
    /// `Return`, so pushing anything here would leave a stray value on the caller's stack.
    fn invoke(
        &mut self,
        method: Value,
        receiver: Value,
        arguments: Vec<Value>,
        instruction: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        match method {
            // The same rule as in `call_value`: a mutating array method given a proxy has to
            // write through it, not into a snapshot.
            Value::Native(native)
                if native.mutates_an_array() && matches!(receiver, Value::Proxy(_)) =>
            {
                self.mutate_through_proxy(native, &receiver, &arguments, instruction)
                    .map(Some)
            }
            Value::Native(native) => {
                // A typed array reached through one of the generic array methods walks as its
                // elements, the same coercion `call_value` applies to a redirected one.
                let original = receiver.clone();
                let receiver = if native.walks_an_array() {
                    self.as_array_like(&Value::Native(native), receiver, instruction)?
                } else {
                    receiver
                };
                let produced = self.dispatch_native(native, &receiver, &arguments, instruction)?;
                Ok(Some(self.typed_result(
                    &original,
                    produced,
                    native.rearranges_in_place(),
                    native.returns_a_new_array(),
                )))
            }
            // `next` on an async generator answers with a promise rather than a result: the
            // body may await any number of times before it reaches the value the consumer asked
            // for, and none of that can happen inside this call.
            Value::Callback(CallbackMethod::AsyncGeneratorNext) => {
                let Value::Generator(generator) = &receiver else {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: receiver.type_name(),
                    });
                };
                let generator = Rc::clone(generator);
                let sent = arguments.first().cloned().unwrap_or(Value::Undefined);
                let step = PromiseState::pending();
                self.drive_async_generator(&generator, &step, Resume::Send(sent))?;
                Ok(Some(Value::Promise(step)))
            }
            // Generator methods drive the machine, so they cannot go through `array_callback`.
            Value::Callback(
                method @ (CallbackMethod::GeneratorNext | CallbackMethod::GeneratorReturn),
            ) => {
                let Value::Generator(generator) = &receiver else {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: receiver.type_name(),
                    });
                };
                let generator = Rc::clone(generator);
                let sent = arguments.first().cloned().unwrap_or(Value::Undefined);
                let (value, done) = if matches!(method, CallbackMethod::GeneratorReturn) {
                    let started = generator.borrow().started;
                    if started {
                        // A started generator has to unwind: every `finally` between the
                        // suspension point and the top runs before it is done.
                        self.resume_generator_with(&generator, Resume::Return(sent), instruction)?
                    } else {
                        // Never entered, so there is nothing to unwind.
                        generator.borrow_mut().done = true;
                        (sent, true)
                    }
                } else {
                    self.resume_generator(&generator, sent, instruction)?
                };
                Ok(Some(iteration_result(value, done)))
            }
            // `reduce` threads an accumulator and `sort` compares pairs, so neither is the walk
            // the other callback methods share. All three coerce their receiver first, for the
            // same reason the native array methods do.
            Value::Callback(CallbackMethod::Reduce | CallbackMethod::ReduceRight) => {
                let backwards = matches!(method, Value::Callback(CallbackMethod::ReduceRight));
                let receiver = self.as_array_like(&method, receiver, instruction)?;
                self.array_reduce(&receiver, &arguments, backwards, instruction)
                    .map(Some)
            }
            Value::Callback(
                callback @ (CallbackMethod::FindLast | CallbackMethod::FindLastIndex),
            ) => {
                let receiver = self.as_array_like(&method, receiver, instruction)?;
                self.array_callback_backwards(callback, &receiver, &arguments, instruction)
                    .map(Some)
            }
            Value::Callback(CallbackMethod::Sort) => {
                let original = receiver.clone();
                let receiver = self.as_array_like(&method, receiver, instruction)?;
                let produced = self.array_sort(&receiver, &arguments, instruction)?;
                Ok(Some(self.typed_result(&original, produced, true, false)))
            }
            Value::Callback(callback) => {
                let original = receiver.clone();
                let receiver = self.as_array_like(&method, receiver, instruction)?;
                let produced = self.array_callback(callback, &receiver, &arguments, instruction)?;
                // `map` and `filter` on a view produce a view of the same kind; the other walks
                // produce a value that is not an array and pass through untouched.
                Ok(Some(self.typed_result(
                    &original,
                    produced,
                    false,
                    matches!(callback, CallbackMethod::Map | CallbackMethod::Filter),
                )))
            }
            // A bound function ignores the receiver the call site offers, whichever way it is
            // reached, so both go through one path.
            Value::Settler(..) | Value::Bound(_) => self
                .call_value(&method, receiver, arguments, instruction)
                .map(Some),
            Value::Function(closure) => {
                // The receiver becomes `this`. Pushing a placeholder plus the arguments is how
                // the shared call path receives them; it pops the placeholder as the callee.
                self.stack.push(Value::Undefined);
                let argc = arguments.len();
                for argument in arguments {
                    self.stack.push(argument);
                }
                self.call_with_this(closure, argc as u16, instruction, receiver, None)?;
                Ok(None)
            }
            other => Err(ExecutionError::NotCallable {
                instruction,
                value: other.type_name(),
            }),
        }
    }

    /// Pop `argc` arguments and the callee beneath them.
    fn take_arguments(
        &mut self,
        argc: u16,
        instruction: usize,
    ) -> Result<Vec<Value>, ExecutionError> {
        let argc = argc as usize;
        if self.stack.len() < argc + 1 {
            return Err(ExecutionError::StackUnderflow { instruction });
        }
        let arguments = self.stack.split_off(self.stack.len() - argc);
        self.stack.pop();
        Ok(arguments)
    }

    /// `object[key]`. Array indices are numeric; everything else is a string key.
    /// Whether an array slot is a hole rather than a value.
    fn is_hole(value: &Value) -> bool {
        matches!(value, Value::Native(Native::Hole))
    }

    fn computed(&self, target: &Value, key: &Value) -> Value {
        if let Value::Array(items) = target
            && let Some(index) = array_index(key)
        {
            // A hole *reads* as undefined even though it is not one, which is why the marker
            // never escapes into an expression.
            return match items.borrow().get(index) {
                Some(value) if !Self::is_hole(value) => value.clone(),
                _ => Value::Undefined,
            };
        }
        if let Value::TypedArray(view) = target
            && let Some(index) = array_index(key)
        {
            // Out of range is `undefined`, and never an ordinary property: a typed array's
            // indices are its whole contents.
            return view.get(index).map_or(Value::Undefined, Value::Number);
        }
        // `s[0]` is the character at that index, as a one-character string. Indices are UTF-16
        // code units, which is the unit `length` counts in, so the two agree on what is in range.
        if let Value::String(text) = target
            && let Some(index) = array_index(key)
        {
            let units: Vec<u16> = text.encode_utf16().collect();
            return match units.get(index) {
                // One unit, not one character: indexing a surrogate pair gives half of it, which
                // is the language's answer and the reason `for-of` over a string differs.
                Some(unit) => Value::String(String::from_utf16_lossy(&[*unit])),
                None => Value::Undefined,
            };
        }
        self.property_key(target, &key.to_property_key())
    }

    /// `delete target[key]`, running a proxy's `deleteProperty` trap when there is one.
    fn delete(
        &mut self,
        target: &Value,
        key: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        let Value::Proxy(proxy) = target else {
            return Ok(delete_property(target, key));
        };
        let proxy = Rc::clone(proxy);
        let inner = proxy.target.clone();
        match self.proxy_trap(&proxy, "deleteProperty", instruction)? {
            Some(trap) => {
                let handler = proxy.handler.clone();
                let arguments = vec![inner, Self::trap_key(key)];
                let answered = self.call_value(&trap, handler, arguments, instruction)?;
                Ok(answered.is_truthy())
            }
            None => self.delete(&inner, key, instruction),
        }
    }

    /// The own enumerable string keys of a value, running a proxy's `ownKeys` trap.
    fn own_keys(
        &mut self,
        target: &Value,
        instruction: usize,
    ) -> Result<Vec<PropertyKey>, ExecutionError> {
        let Value::Proxy(proxy) = target else {
            return Ok(own_property_keys(target));
        };
        let proxy = Rc::clone(proxy);
        let inner = proxy.target.clone();
        match self.proxy_trap(&proxy, "ownKeys", instruction)? {
            Some(trap) => {
                let handler = proxy.handler.clone();
                let listed = self.call_value(&trap, handler, vec![inner], instruction)?;
                Ok(match listed {
                    Value::Array(items) => dense_items(&items.borrow())
                        .into_iter()
                        .map(|key| key.to_property_key())
                        .collect(),
                    _ => Vec::new(),
                })
            }
            None => self.own_keys(&inner, instruction),
        }
    }

    fn proxy_property_is_enumerable(
        &mut self,
        target: &Value,
        key: &PropertyKey,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        let descriptor = self.proxy_descriptor(target, &key_value(key), instruction)?;
        if descriptor.is_nullish() {
            return Ok(false);
        }
        Ok(self
            .read_property(&descriptor, "enumerable", instruction)?
            .is_truthy())
    }

    fn enumerable_own_properties(
        &mut self,
        target: &Value,
        instruction: usize,
    ) -> Result<Vec<(PropertyKey, Property)>, ExecutionError> {
        if matches!(target, Value::Proxy(_)) {
            let mut properties = Vec::new();
            for key in self.own_keys(target, instruction)? {
                if self.proxy_property_is_enumerable(target, &key, instruction)? {
                    properties.push((
                        key.clone(),
                        Property::Value(self.read_property(target, &key, instruction)?),
                    ));
                }
            }
            Ok(properties)
        } else {
            Ok(own_properties(target))
        }
    }

    /// Does `target` have `key`, running a proxy's `has` trap.
    fn has(
        &mut self,
        target: &Value,
        key: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        let Value::Proxy(proxy) = target else {
            return Ok(has_property(target, key));
        };
        let proxy = Rc::clone(proxy);
        let inner = proxy.target.clone();
        match self.proxy_trap(&proxy, "has", instruction)? {
            Some(trap) => {
                let handler = proxy.handler.clone();
                let arguments = vec![inner, Self::trap_key(key)];
                let answered = self.call_value(&trap, handler, arguments, instruction)?;
                Ok(answered.is_truthy())
            }
            None => self.has(&inner, key, instruction),
        }
    }

    /// A computed read that can run a proxy's `get` trap.
    ///
    /// `computed` itself is pure and cannot call anything, which is fine for every other kind of
    /// value and not fine for a proxy: reading one *is* a call.
    fn read_computed(
        &mut self,
        target: &Value,
        key: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if matches!(target, Value::Proxy(_)) {
            return self.proxy_get(target, key, instruction);
        }
        let property_key = key.to_property_key();
        let accessor = match target {
            Value::Object(object) => object.borrow().lookup(property_key.clone()),
            Value::Function(closure)
                if property_key != PropertyKey::String("prototype".to_owned()) =>
            {
                closure.properties.borrow().lookup(property_key.clone())
            }
            Value::Array(items) => items.properties.borrow().lookup(property_key.clone()),
            _ => None,
        };
        match accessor {
            Some(Property::Accessor { get, .. }) => match get {
                Some(getter) => self.call_value(&getter, target.clone(), Vec::new(), instruction),
                None => Ok(Value::Undefined),
            },
            _ => Ok(self.computed(target, key)),
        }
    }

    /// Read `key` from a proxy, through its `get` trap or through its target.
    fn proxy_get(
        &mut self,
        target: &Value,
        key: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Proxy(proxy) = target else {
            return self.read_computed(target, key, instruction);
        };
        let proxy = Rc::clone(proxy);
        let inner = proxy.target.clone();
        match self.proxy_trap(&proxy, "get", instruction)? {
            Some(trap) => {
                let handler = proxy.handler.clone();
                let arguments = vec![inner, Self::trap_key(key), target.clone()];
                self.call_value(&trap, handler, arguments, instruction)
            }
            None => self.read_computed(&inner, key, instruction),
        }
    }

    fn set_computed(
        &mut self,
        target: &Value,
        key: &Value,
        value: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        if let Value::Proxy(proxy) = target {
            let proxy = Rc::clone(proxy);
            let inner = proxy.target.clone();
            return match self.proxy_trap(&proxy, "set", instruction)? {
                Some(trap) => {
                    let handler = proxy.handler.clone();
                    let arguments = vec![inner, Self::trap_key(key), value, target.clone()];
                    self.call_value(&trap, handler, arguments, instruction)?;
                    Ok(())
                }
                None => self.set_computed(&inner, key, value, instruction),
            };
        }
        if let Value::TypedArray(view) = target {
            if let Some(index) = array_index(key) {
                view.set(index, value.as_number());
                return Ok(());
            }
            // A named property of a typed array is silently discarded rather than stored: the
            // view has nowhere to put one, and inventing a store would make `u8.x = 1` look like
            // it worked.
            return Ok(());
        }
        match target {
            // A frozen array ignores an element write, the same way a frozen object ignores a
            // property write. Sloppy mode discards it and strict mode throws; the machine does
            // not know which it is in, and discarding is the one that cannot invent an error.
            Value::Array(items) if items.properties.borrow().is_frozen() => Ok(()),
            Value::Array(array) => match array_index(key) {
                Some(index) => {
                    let mut values = array.borrow_mut();
                    let was_present = values
                        .get(index)
                        .is_some_and(|item| !matches!(item, Value::Native(Native::Hole)));
                    // Writing past the end extends with *holes*, not undefined: `a[3] = 4` on a
                    // one-element array leaves two positions that `in` denies, `for...in` skips
                    // and a console shows as `<2 empty items>`.
                    if index >= values.len() {
                        values.resize(index + 1, Value::Native(Native::Hole));
                    }
                    values[index] = value;
                    drop(values);
                    if !was_present {
                        // Assignment creates a fresh enumerable property when it fills a hole.
                        array.remove_index_attributes(index);
                    }
                    Ok(())
                }
                // `length` is derived, not stored: writing it truncates or extends. This is how
                // a program empties an array in place -- `arr.length = 0` -- and storing it as an
                // ordinary property left the array untouched while the write appeared to work.
                None if matches!(key.to_property_key(), PropertyKey::String(ref name) if name == "length") =>
                {
                    let length = value.as_number();
                    // A length that is not a whole number in range is a RangeError, not a
                    // silently ignored write: `arr.length = -1` is a bug in the program.
                    if !length.is_finite() || length < 0.0 || length.fract() != 0.0 {
                        return Err(ExecutionError::InvalidArrayLength);
                    }
                    {
                        let length = length as usize;
                        let mut values = array.borrow_mut();
                        if length <= values.len() {
                            values.truncate(length);
                        } else {
                            // Growing leaves holes, which read as undefined and are not the same
                            // thing: `1 in a` is false for one.
                            values.resize(length, Value::Native(Native::Hole));
                        }
                        // A descriptor for an index beyond the new length no longer describes
                        // an own property and must not affect a later re-use of that index.
                    }
                    array.truncate_index_attributes(length as usize);
                    Ok(())
                }
                // Not an index, so it is an ordinary named property of the array.
                None => {
                    let name = key.to_property_key();
                    if self.inherited_setter(
                        &array.properties,
                        target,
                        &name,
                        &value,
                        instruction,
                    )? {
                        return Ok(());
                    }
                    array.properties.borrow_mut().set(&name, value);
                    Ok(())
                }
            },
            // `lastIndex` is the one writable property a regexp has, and writing it is how a
            // program drives a sticky or global search by hand.
            Value::RegExp(regexp) => {
                if key.property_key() == "lastIndex" {
                    let index = value.as_number();
                    regexp.last_index.set(if index.is_finite() && index > 0.0 {
                        index as usize
                    } else {
                        0
                    });
                }
                Ok(())
            }
            Value::Object(object) => {
                let name = key.to_property_key();
                if self.inherited_setter(object, target, &name, &value, instruction)? {
                    return Ok(());
                }
                object.borrow_mut().set(name, value);
                Ok(())
            }
            Value::Function(closure) => {
                let name = key.to_property_key();
                // `f.prototype = {...}` replaces the object `new f()` will link instances to,
                // which is a slot on the closure rather than an ordinary property -- so a write
                // that landed in the property store would be read back by `f.prototype` and
                // ignored by `new`.
                if name == PropertyKey::String("prototype".to_owned()) {
                    *closure.prototype.borrow_mut() = Some(value);
                    return Ok(());
                }
                if self.inherited_setter(&closure.properties, target, &name, &value, instruction)? {
                    return Ok(());
                }
                closure.properties.borrow_mut().set(name, value);
                Ok(())
            }
            other => Err(ExecutionError::NotAnObject {
                instruction,
                value: other.type_name(),
            }),
        }
    }

    fn string_constant(&self, index: u32, instruction: usize) -> Result<String, ExecutionError> {
        match self.program.constants.get(index as usize) {
            Some(Constant::String(name)) => Ok(name.clone()),
            Some(_) => Err(ExecutionError::InvalidPropertyKey {
                instruction,
                value: "non-string constant",
            }),
            None => Err(ExecutionError::ConstantOutOfBounds {
                index: index as usize,
            }),
        }
    }

    /// The cell at this closure's upvalue index.
    fn upvalue(&self, index: u32) -> Result<Cell, ExecutionError> {
        self.function
            .as_ref()
            .and_then(|closure| closure.upvalues.get(index as usize))
            .cloned()
            .ok_or(ExecutionError::MissingUpvalue { index })
    }

    /// The cell held in a local slot, for capture.
    fn cell(&self, slot: usize) -> Result<Cell, ExecutionError> {
        match self.locals.get(slot) {
            Some(Value::Cell(cell)) => Ok(Rc::clone(cell)),
            // `NewCell` runs before anything can capture the slot, so a non-cell here means the
            // compiler emitted a capture of a binding it never boxed.
            Some(_) | None => Err(ExecutionError::NotACell { slot }),
        }
    }

    /// Enter `function` with `argc` arguments sitting on top of the stack.
    ///
    /// Missing arguments become `undefined` and extra ones are discarded, which is what calling
    /// a JavaScript function with the wrong arity does. Neither is an error, so neither is
    /// reported as one.
    fn call(
        &mut self,
        closure: Rc<Closure>,
        argc: u16,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        self.call_with_this(closure, argc, instruction, Value::Undefined, None)
    }

    /// Enter a closure with an explicit receiver.
    fn call_with_this(
        &mut self,
        closure: Rc<Closure>,
        argc: u16,
        instruction: usize,
        this_value: Value,
        constructing: Option<Value>,
    ) -> Result<(), ExecutionError> {
        let function = closure.function;
        let Some(proto) = closure.program.functions.get(function as usize) else {
            return Err(ExecutionError::UnknownFunction { function });
        };
        // Resolve the effective lexical call state before splitting ordinary and suspended
        // functions. Async functions use the generator machinery, but an async arrow still has
        // exactly the same captured `this` and `new.target` as an ordinary arrow.
        let entering = match &closure.captured_this {
            Some(captured) => captured.clone(),
            None => this_value,
        };
        let target = match &closure.captured_new_target {
            Some(captured) => captured.clone(),
            None if constructing.is_some() => Value::Function(Rc::clone(&closure)),
            None => Value::Undefined,
        };
        if proto.generator {
            // Calling a generator function does not run it: it captures the arguments and hands
            // back an object whose `next` runs the body up to each `yield`.
            // An async generator is called like a plain one: it hands back an object, and
            // nothing runs until the consumer asks for a step.
            let asynchronous = proto.asynchronous && !proto.async_generator;
            let generator = self.build_generator(&closure, argc, instruction, entering, target)?;
            if !asynchronous {
                self.stack.push(generator);
                return Ok(());
            }
            // An async function *does* run, up to its first `await`, and hands back a promise
            // rather than the generator -- which is the only difference the caller sees.
            let Value::Generator(generator) = generator else {
                unreachable!("`build_generator` produces a generator");
            };
            let promise = PromiseState::pending();
            self.drive_async(&generator, &promise, Resume::Send(Value::Undefined))?;
            self.stack.push(Value::Promise(promise));
            return Ok(());
        }
        if self.frames.len() >= self.limits.max_call_depth {
            return Err(ExecutionError::CallDepthExceeded {
                limit: self.limits.max_call_depth,
            });
        }

        let argc = argc as usize;
        if self.stack.len() < argc + 1 {
            return Err(ExecutionError::StackUnderflow { instruction });
        }
        let mut arguments = self.stack.split_off(self.stack.len() - argc);
        // The callee sits directly below its arguments; removing it here is what makes the
        // stack look, to the caller, as though the call left only its result.
        self.stack.pop();
        let received = arguments.clone();

        let mut locals = vec![Value::Undefined; proto.local_count as usize];
        arguments.truncate(proto.parameter_count as usize);
        for (slot, argument) in arguments.into_iter().enumerate() {
            locals[slot] = argument;
        }

        // The frame keeps what this call was actually given, whatever the closure is: an arrow's
        // rest parameter needs it, and `LoadArguments` is the one place that substitutes the
        // captured list instead.
        self.frames.push(Frame {
            program: std::mem::replace(&mut self.program, Rc::clone(&closure.program)),
            function: self.function.take(),
            this_value: std::mem::replace(&mut self.this_value, entering),
            new_target: std::mem::replace(&mut self.new_target, target),
            arguments: std::mem::replace(&mut self.arguments, received),
            constructing,
            locals: std::mem::replace(&mut self.locals, locals),
            instruction: self.instruction,
            stack_base: self.stack.len(),
        });
        self.function = Some(closure);
        self.instruction = 0;
        Ok(())
    }

    fn pop(&mut self, instruction: usize) -> Result<Value, ExecutionError> {
        self.stack
            .pop()
            .ok_or(ExecutionError::StackUnderflow { instruction })
    }

    fn jump(&mut self, instruction: usize, target: u32) -> Result<(), ExecutionError> {
        let target = target as usize;
        if target >= Self::code(&self.program, self.function.as_ref()).len() {
            return Err(ExecutionError::InvalidJump {
                instruction,
                target,
            });
        }
        self.instruction = target;
        Ok(())
    }
}

fn unary(operator: UnaryOp, value: Value) -> Value {
    // A BigInt stays one under negation and bitwise-not; `+x` is the exception, and is a
    // TypeError in the specification precisely so that `+` cannot silently narrow one.
    if let Value::BigInt(value) = &value {
        match operator {
            UnaryOp::Negate => return Value::BigInt(Rc::new(-value.as_ref().clone())),
            UnaryOp::BitwiseNot => return Value::BigInt(Rc::new(!value.as_ref().clone())),
            _ => {}
        }
    }
    match operator {
        UnaryOp::Plus => Value::Number(value.as_number()),
        UnaryOp::Negate => Value::Number(-value.as_number()),
        UnaryOp::Not => Value::Boolean(!value.is_truthy()),
        // `typeof` never coerces and never throws; it reports the type tag directly.
        UnaryOp::TypeOf => Value::String(value.type_of().to_owned()),
        // `~x` is `-(ToInt32(x) + 1)`: a numeric operator, despite reading as a logical one.
        UnaryOp::BitwiseNot => Value::Number(f64::from(!to_int32(value.as_number()))),
        // `void x` evaluates its operand -- which already happened -- and discards it.
        UnaryOp::Void => Value::Undefined,
    }
}

/// `ToInt32`: the truncation every bitwise operator applies to its operands first.
///
/// This is why `1 << 31` is negative and why `2 ** 32 | 0` is zero. Not a cast: the value wraps
/// modulo 2^32 rather than saturating, and anything not finite becomes zero.
/// The seed `Math.random` starts from, and the constants that advance it.
///
/// SplitMix64: a full-period generator with good statistical properties in five lines, which is
/// what a language builtin needs. The seed is a constant so a run is reproducible; see ADR 0026
/// for why that is a decision rather than a shortcut.
const RANDOM_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

fn split_mix_64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut mixed = *state;
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    mixed ^ (mixed >> 31)
}

/// A double in `[0, 1)` from 53 random bits, which is every bit the mantissa has.
fn random_double(state: &mut u64) -> f64 {
    #[expect(clippy::cast_precision_loss)]
    let bits = (split_mix_64(state) >> 11) as f64;
    bits / 9_007_199_254_740_992.0
}

fn to_int32(value: f64) -> i32 {
    to_uint32(value) as i32
}

/// `ToUint32`, the same truncation read as unsigned. `>>>` is the only operator that uses it
/// directly, which is what makes `x >>> 0` the idiom for reading a number as unsigned.
fn to_uint32(value: f64) -> u32 {
    if !value.is_finite() || value == 0.0 {
        return 0;
    }
    let truncated = value.trunc();
    // `rem_euclid` gives the non-negative remainder, which is the wrap the specification asks
    // for; `as u32` alone saturates instead, so `2 ** 32` would come out as u32::MAX.
    (truncated.rem_euclid(4_294_967_296.0)) as u32
}

/// Arithmetic on two BigInts, or `None` when this is not a BigInt operation.
///
/// Returns `Err` when one side is a BigInt and the other is not: the language refuses that
/// deliberately, since silently widening either way is how precision is lost.
fn bigint_binary(
    operator: BinaryOp,
    left: &Value,
    right: &Value,
) -> Option<Result<Value, ExecutionError>> {
    use num_bigint::BigInt as Big;
    let (a, b) = match (left, right) {
        (Value::BigInt(a), Value::BigInt(b)) => (a.as_ref().clone(), b.as_ref().clone()),
        // Comparison across the two *is* allowed -- only arithmetic is not -- and so is `==`,
        // both of which are handled by the ordinary paths on the numeric value.
        (Value::BigInt(_), _) | (_, Value::BigInt(_)) => {
            return match operator {
                BinaryOp::Add
                | BinaryOp::Subtract
                | BinaryOp::Multiply
                | BinaryOp::Divide
                | BinaryOp::Remainder
                | BinaryOp::Exponentiate => Some(Err(ExecutionError::MixedBigInt)),
                _ => None,
            };
        }
        _ => return None,
    };
    let zero = Big::from(0);
    Some(Ok(match operator {
        BinaryOp::Add => Value::BigInt(Rc::new(a + b)),
        BinaryOp::Subtract => Value::BigInt(Rc::new(a - b)),
        BinaryOp::Multiply => Value::BigInt(Rc::new(a * b)),
        // Division truncates toward zero and by zero is a RangeError, neither of which matches
        // what a number would do.
        BinaryOp::Divide | BinaryOp::Remainder => {
            if b == zero {
                return Some(Err(ExecutionError::RadixOutOfRange));
            }
            Value::BigInt(Rc::new(if matches!(operator, BinaryOp::Divide) {
                a / b
            } else {
                a % b
            }))
        }
        BinaryOp::Exponentiate => {
            let Some(exponent) = num_traits::ToPrimitive::to_u32(&b) else {
                return Some(Err(ExecutionError::RadixOutOfRange));
            };
            Value::BigInt(Rc::new(num_traits::Pow::pow(a, exponent)))
        }
        BinaryOp::BitwiseAnd => Value::BigInt(Rc::new(a & b)),
        BinaryOp::BitwiseOr => Value::BigInt(Rc::new(a | b)),
        BinaryOp::BitwiseXor => Value::BigInt(Rc::new(a ^ b)),
        BinaryOp::StrictEqual | BinaryOp::LooseEqual => Value::Boolean(a == b),
        BinaryOp::StrictNotEqual | BinaryOp::LooseNotEqual => Value::Boolean(a != b),
        BinaryOp::LessThan => Value::Boolean(a < b),
        BinaryOp::LessThanOrEqual => Value::Boolean(a <= b),
        BinaryOp::GreaterThan => Value::Boolean(a > b),
        BinaryOp::GreaterThanOrEqual => Value::Boolean(a >= b),
        _ => return None,
    }))
}

fn binary(operator: BinaryOp, left: Value, right: Value) -> Value {
    match operator {
        BinaryOp::Add => {
            if matches!(left, Value::String(_)) || matches!(right, Value::String(_)) {
                Value::String(format!(
                    "{}{}",
                    left.to_display_string(),
                    right.to_display_string()
                ))
            } else {
                Value::Number(left.as_number() + right.as_number())
            }
        }
        BinaryOp::Subtract => Value::Number(left.as_number() - right.as_number()),
        BinaryOp::Multiply => Value::Number(left.as_number() * right.as_number()),
        BinaryOp::Divide => Value::Number(left.as_number() / right.as_number()),
        BinaryOp::Remainder => Value::Number(left.as_number() % right.as_number()),
        BinaryOp::Exponentiate => Value::Number(left.as_number().powf(right.as_number())),
        BinaryOp::StrictEqual => Value::Boolean(strict_equal(&left, &right)),
        // `key in object` tests presence, which is not the same question as reading the key and
        // comparing to undefined: a property explicitly set to undefined is present.
        BinaryOp::In => Value::Boolean(has_property(&right, &left)),
        // Handled in the interpreter loop, which has the realm's Error.prototype; reaching here
        // would mean the loop stopped special-casing it.
        BinaryOp::InstanceOf => unreachable!("instanceof is evaluated with realm context"),

        BinaryOp::StrictNotEqual => Value::Boolean(!strict_equal(&left, &right)),
        BinaryOp::LooseEqual => Value::Boolean(loose_equal(&left, &right)),
        BinaryOp::LooseNotEqual => Value::Boolean(!loose_equal(&left, &right)),
        // Every bitwise operator truncates to 32 bits first, so `1 << 31` is negative.
        BinaryOp::BitwiseAnd => Value::Number(f64::from(
            to_int32(left.as_number()) & to_int32(right.as_number()),
        )),
        BinaryOp::BitwiseOr => Value::Number(f64::from(
            to_int32(left.as_number()) | to_int32(right.as_number()),
        )),
        BinaryOp::BitwiseXor => Value::Number(f64::from(
            to_int32(left.as_number()) ^ to_int32(right.as_number()),
        )),
        // Only the low five bits of the shift count are used, so `1 << 32` is 1, not 0.
        BinaryOp::ShiftLeft => Value::Number(f64::from(
            to_int32(left.as_number()).wrapping_shl(to_uint32(right.as_number()) & 31),
        )),
        BinaryOp::ShiftRight => Value::Number(f64::from(
            to_int32(left.as_number()).wrapping_shr(to_uint32(right.as_number()) & 31),
        )),
        // The one shift that reads its left operand as unsigned, and so the one whose result can
        // exceed i32::MAX -- `(1 << 31) >>> 0` is 2147483648.
        BinaryOp::UnsignedShiftRight => Value::Number(f64::from(
            to_uint32(left.as_number()).wrapping_shr(to_uint32(right.as_number()) & 31),
        )),
        BinaryOp::LessThan => Value::Boolean(compare(&left, &right, OrderingPredicate::Less)),
        BinaryOp::LessThanOrEqual => {
            Value::Boolean(compare(&left, &right, OrderingPredicate::LessEqual))
        }
        BinaryOp::GreaterThan => Value::Boolean(compare(&left, &right, OrderingPredicate::Greater)),
        BinaryOp::GreaterThanOrEqual => {
            Value::Boolean(compare(&left, &right, OrderingPredicate::GreaterEqual))
        }
    }
}

/// Build an `Error` instance.
///
/// `stack` is a header line and nothing more. A release build strips unwind tables and
/// dead-strips symbols, so real frames are not obtainable, and `Error.stackTraceLimit = 0` in V8
/// produces exactly this shape -- a truthful degenerate case rather than an invented trace.
///
/// `cause` follows InstallErrorCause: the property exists when the options object *has* a
/// `cause` key, including when its value is `undefined`. Testing presence rather than value is
/// the whole point of that step in the specification.
fn build_error(kind: ErrorKind, arguments: &[Value]) -> Value {
    let message = match arguments.first() {
        None | Some(Value::Undefined) => String::new(),
        Some(value) => value.to_display_string(),
    };

    let name = kind.name();
    let mut data = ObjectData::default();
    // Every own property of an error is non-enumerable, which is why `JSON.stringify(err)` is
    // `{}` and `Object.keys(err)` is empty -- an error is meant to be *read*, not walked, and a
    // program that serialises one is asking for the fields it names rather than all of them.
    data.set_non_enumerable("message", Value::String(message.clone()));
    data.set_non_enumerable("name", Value::String(name.to_owned()));
    data.set_non_enumerable(
        "stack",
        Value::String(if message.is_empty() {
            name.to_owned()
        } else {
            format!("{name}: {message}")
        }),
    );
    if let Some(Value::Object(options)) = arguments.get(1)
        && let Some(cause) = options.borrow().get("cause")
    {
        data.set_non_enumerable("cause", cause.clone());
    }
    Value::Object(Rc::new(RefCell::new(data)))
}

/// The `space` argument of `JSON.stringify`, resolved to the literal indent for one level.
///
/// A number is that many spaces and a string is used as-is, both capped at ten characters, which
/// is what the specification says and what keeps a hostile argument from blowing up the output.
fn json_indent(space: Option<&Value>) -> String {
    const MAX: usize = 10;
    match space {
        Some(Value::Number(count)) if *count >= 1.0 => " ".repeat((*count as usize).min(MAX)),
        Some(Value::String(text)) => text.chars().take(MAX).collect(),
        _ => String::new(),
    }
}

/// Serialise one value as JSON.
///
/// `None` is "not representable" -- `undefined`, a function, a symbol. The distinction matters
/// because the caller decides what to do with it: an object property is dropped, an array
/// element becomes `null`, and at the top level the whole call returns `undefined`.
///
/// `resolve` calls a getter, because reading a property is a call when the property is an
/// accessor. It is a callback rather than the interpreter itself so that the serialiser stays a
/// free function -- the shape of the output is not a machine concern.
///
/// Not supported yet: the `replacer` argument and `toJSON` hooks, both of which call back into
/// the program at points this signature does not reach. Neither appears in the
/// corpus; when one does, this grows a `&mut Interpreter` and moves onto the type.
/// The one callback `json_stringify` needs, doing both jobs it can require of the machine.
///
/// With a getter it calls that getter; with `None` it performs the `toJSON` step -- look the name
/// up on the value, call it if it is callable, and hand back what it produced, or the value
/// unchanged when there is none. One closure rather than two because both borrow the interpreter
/// mutably and only one of them can.
type JsonResolver<'a> = &'a mut dyn FnMut(Option<&Value>, &Value) -> Result<Value, ExecutionError>;

/// Serialise one value, calling back into the interpreter where the specification says to.
///
fn json_stringify(
    value: &Value,
    indent: &str,
    current: &str,
    seen: &mut Vec<usize>,
    resolve: JsonResolver<'_>,
) -> Result<Option<String>, ExecutionError> {
    // `toJSON` runs before anything else is decided about the value, which is how a `Date`
    // serialises as its ISO string and how a library controls its own JSON form. It is looked
    // up on the value, prototype chain included, so a class method counts.
    let replaced = resolve(None, value)?;
    let value = &replaced;
    let nested = format!("{current}{indent}");
    // With no indent every separator is empty, which collapses the pretty form to the compact
    // one and lets both share this single code path.
    let (open, separator, close) = if indent.is_empty() {
        (String::new(), ",".to_owned(), String::new())
    } else {
        (
            format!("\n{nested}"),
            format!(",\n{nested}"),
            format!("\n{current}"),
        )
    };
    let colon = if indent.is_empty() { ":" } else { ": " };

    Ok(match value {
        // Omitted rather than serialised: JSON has no way to spell any of these. An array
        // element that is omitted becomes null, which is also what a hole serialises as.
        // A method table serialises as the empty object it stands for; a function serialises as
        // nothing, which is what drops it from an object and turns it into null in an array.
        Value::Native(Native::PrototypeOf(table)) if *table != PrototypeTable::Function => {
            Some("{}".to_owned())
        }
        Value::Undefined | Value::Function(_) | Value::Native(_) | Value::Callback(_) => None,
        // A symbol is dropped like a function, unlike `String(symbol)`, which throws.
        Value::Symbol(_) => None,
        Value::Null => Some("null".to_owned()),
        Value::Boolean(value) => Some(value.to_string()),
        // NaN and both infinities have no JSON spelling, so they serialise as null.
        Value::Number(number) if number.is_finite() => Some(number_string(*number)),
        Value::Number(_) => Some("null".to_owned()),
        Value::String(text) => Some(json_quote(text)),
        Value::Array(items) => {
            let guard = CycleGuard::enter(seen, Rc::as_ptr(items).cast::<()>())?;
            let items = items.borrow().clone();
            let parts = items
                .iter()
                .map(|item| {
                    // An element that is not representable becomes null rather than vanishing,
                    // because dropping it would renumber every element after it.
                    json_stringify(item, indent, &nested, guard.seen, resolve)
                        .map(|part| part.unwrap_or_else(|| "null".to_owned()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Some(if parts.is_empty() {
                "[]".to_owned()
            } else {
                format!("[{open}{}{close}]", parts.join(&separator))
            })
        }
        Value::Object(object) => {
            let guard = CycleGuard::enter(seen, Rc::as_ptr(object).cast::<()>())?;
            // Read out before iterating, so the object is not borrowed while a getter runs --
            // a getter is ordinary code and may touch the object it belongs to.
            let entries = own_properties(value);
            let mut parts = Vec::new();
            for (key, property) in entries {
                // JSON only includes enumerable string keys. Symbols and VM-private slots are
                // intentionally invisible, while an ordinary string beginning with `@@` is not.
                let Some(key) = string_key(&key) else {
                    continue;
                };
                let property = match property {
                    Property::Value(value) => value,
                    // The specification reads the property, which for an accessor is a call.
                    // Serialising the getter itself would be a different object entirely.
                    Property::Accessor { get, .. } => match get {
                        Some(getter) => resolve(Some(&getter), value)?,
                        None => continue,
                    },
                };
                if let Some(part) = json_stringify(&property, indent, &nested, guard.seen, resolve)?
                {
                    parts.push(format!("{}{colon}{part}", json_quote(key)));
                }
            }
            let _ = object;
            Some(if parts.is_empty() {
                "{}".to_owned()
            } else {
                format!("{{{open}{}{close}}}", parts.join(&separator))
            })
        }
        // None of these has own enumerable properties, so all serialise as the empty object --
        // which is also what a real engine produces for them.
        // A regexp has no own enumerable properties either, so `JSON.stringify(/a/)` is `{}`.
        Value::Generator(_) | Value::Promise(_) | Value::RegExp(_) => Some("{}".to_owned()),
        // A date serialises through `toJSON`, which is its ISO form -- or null when it is invalid,
        // since there is no ISO spelling for that.
        Value::Date(time) => Some(match iso_string(time.get()) {
            Some(text) => json_quote(&text),
            None => "null".to_owned(),
        }),
        // A function has no JSON spelling, and a settler is one.
        Value::Settler(..) | Value::Bound(_) => None,
        // Serialising a proxy means reading its properties, which means running its traps, which
        // this pure walk cannot do. Refused rather than silently serialising the target.
        Value::Proxy(_) => return Err(ExecutionError::ProxyNotSerialisable),
        // A typed array serialises as an *object* keyed by index, not as an array, which is one
        // of the specification's odder corners and is what node does.
        Value::TypedArray(view) => {
            let entries: Vec<(String, Value)> = view
                .items()
                .into_iter()
                .enumerate()
                .map(|(index, item)| (index.to_string(), item))
                .collect();
            let mut data = ObjectData::default();
            for (key, item) in entries {
                data.set(&key, item);
            }
            return json_stringify(
                &Value::Object(Rc::new(RefCell::new(data))),
                indent,
                current,
                seen,
                resolve,
            );
        }
        Value::ArrayBuffer(_) => Some("{}".to_owned()),
        // Neither has a BigInt: the specification makes serialising one a TypeError rather than
        // an omission, and it is fatal here for the same reason a cycle is.
        Value::BigInt(_) => return Err(ExecutionError::CyclicJson),
        Value::Cell(_) => unreachable!("a cell is never used as a value"),
    })
}

/// Tracks the containers on the path from the root, so a value containing itself is caught
/// rather than recursed into forever.
///
/// The identity is the allocation address, which is exactly the identity JavaScript uses. Being
/// a guard rather than a bare push means the entry is removed on the error path too, so the set
/// stays the current *path* and a value legitimately reachable twice is not mistaken for a cycle.
struct CycleGuard<'a> {
    seen: &'a mut Vec<usize>,
}

impl<'a> CycleGuard<'a> {
    fn enter(seen: &'a mut Vec<usize>, container: *const ()) -> Result<Self, ExecutionError> {
        let identity = container as usize;
        if seen.contains(&identity) {
            return Err(ExecutionError::CyclicJson);
        }
        seen.push(identity);
        Ok(Self { seen })
    }
}

impl Drop for CycleGuard<'_> {
    fn drop(&mut self) {
        self.seen.pop();
    }
}

/// A JSON string literal, with the escapes the grammar requires.
fn json_quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '\u{8}' => quoted.push_str("\\b"),
            '\u{c}' => quoted.push_str("\\f"),
            // Everything else below space has no short escape and must be spelled numerically.
            control if control < ' ' => {
                quoted.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// Invoke a native built-in.
///
/// Natives take their receiver explicitly rather than through a `this` binding, because there is
/// no `this` yet. When there is, these become ordinary functions and this dispatch disappears --
/// which is the point of keeping the set small.
/// Whether this value is a frozen array, which the length-changing methods must refuse.
///
/// `push` on a frozen array is a TypeError in *both* modes, unlike an element assignment: the
/// method itself performs a definition, and a definition on a non-extensible object throws
/// whatever the caller's strictness.
fn is_frozen_array(value: &Value) -> bool {
    matches!(value, Value::Array(items) if items.properties.borrow().is_frozen())
}

fn call_native(
    native: Native,
    receiver: &Value,
    arguments: &[Value],
) -> Result<Value, ExecutionError> {
    // Checked once rather than in each of the four, and before any of them has taken a borrow.
    if native.changes_array_length() && is_frozen_array(receiver) {
        return Err(ExecutionError::FrozenArray);
    }
    Ok(match native {
        // A proxy of an array *is* an array: `isArray` looks through as many proxies as it
        // takes, which is what lets a library wrap one and still branch on the shape.
        Native::ArrayIsArray => Value::Boolean(matches!(
            arguments.first().map(unwrap_proxy),
            Some(Value::Array(_))
        )),
        Native::BigIntConstructor => {
            let Some(value) = arguments.first() else {
                return Ok(Value::BigInt(Rc::new(num_bigint::BigInt::from(0))));
            };
            match value {
                Value::BigInt(_) => value.clone(),
                Value::String(text) => match text.trim().parse::<num_bigint::BigInt>() {
                    Ok(parsed) => Value::BigInt(Rc::new(parsed)),
                    // A string that is not an integer is a SyntaxError, unlike `Number("x")`
                    // which is NaN -- a BigInt has no NaN to be.
                    Err(_) => return Err(ExecutionError::InvalidJson),
                },
                other => {
                    let number = other.as_number();
                    // Only an exact integer converts: `BigInt(1.5)` is a RangeError rather than
                    // a rounding, because rounding is the thing this type exists to avoid.
                    if !number.is_finite() || number.fract() != 0.0 {
                        return Err(ExecutionError::RadixOutOfRange);
                    }
                    match num_bigint::BigInt::parse_bytes(
                        format!("{number:.0}").as_bytes(),
                        10,
                    ) {
                        Some(parsed) => Value::BigInt(Rc::new(parsed)),
                        None => return Err(ExecutionError::RadixOutOfRange),
                    }
                }
            }
        }
        Native::BigIntToString => {
            let Value::BigInt(value) = receiver else {
                return Ok(Value::Undefined);
            };
            match arguments.first().map(Value::as_number) {
                None | Some(10.0) => Value::String(value.to_string()),
                Some(radix) if (2.0..=36.0).contains(&radix) => {
                    Value::String(value.to_str_radix(radix as u32))
                }
                _ => return Err(ExecutionError::RadixOutOfRange),
            }
        }
        // A method table's own properties are the methods it holds, which is what a
        // plain-object check reads when it asks whether `Object.prototype` has a `constructor`.
        Native::ObjectHasOwnProperty if matches!(receiver, Value::Native(Native::PrototypeOf(_))) => {
            let Value::Native(table) = receiver else {
                unreachable!("checked by the guard");
            };
            let name = arguments
                .first()
                .map_or_else(String::new, |key| key.clone().property_key());
            Value::Boolean(!matches!(
                table.property(&name),
                None | Some(Value::Undefined)
            ))
        }
        Native::ObjectHasOwnProperty => Value::Boolean(own_property_exists(
            receiver,
            arguments.first().unwrap_or(&Value::Undefined),
        )),
        Native::ObjectIsPrototypeOf => {
            let mut current = arguments.first().cloned().unwrap_or(Value::Undefined);
            loop {
                let parent = match &current {
                    Value::Object(object) => object.borrow().prototype().cloned(),
                    Value::Function(closure) => closure.properties.borrow().prototype().cloned(),
                    _ => None,
                };
                match parent {
                    Some(parent) => {
                        if strict_equal(&parent, receiver) {
                            break Value::Boolean(true);
                        }
                        current = parent;
                    }
                    None => break Value::Boolean(false),
                }
            }
        }
        // Every property this realm stores is enumerable, so this is `hasOwnProperty` under
        // another name until attributes are tracked per property.
        Native::ObjectPropertyIsEnumerable => {
            Value::Boolean(own_enumerable(receiver, arguments.first().unwrap_or(&Value::Undefined)))
        }
        // `Object.prototype.toString.call(x)`, the oldest type test there is.
        Native::ObjectToStringTag => Value::String(format!(
            "[object {}]",
            match receiver {
                Value::Undefined => "Undefined",
                Value::Null => "Null",
                Value::Array(_) => "Array",
                Value::Boolean(_) => "Boolean",
                Value::Number(_) => "Number",
                Value::String(_) => "String",
                Value::Date(_) => "Date",
                Value::RegExp(_) => "RegExp",
                Value::Promise(_) => "Promise",
                Value::TypedArray(view) => view.kind.name(),
                Value::ArrayBuffer(_) => "ArrayBuffer",
                Value::Function(_)
                | Value::Native(_)
                | Value::Callback(_)
                | Value::Settler(..)
                | Value::Bound(_) => "Function",
                _ => "Object",
            }
        )),
        // `Object.prototype.valueOf` returns the receiver unchanged, which is the identity that
        // makes `ToPrimitive` fall through to `toString` for a plain object.
        Native::ObjectValueOf => receiver.clone(),
        Native::PrimitiveToString => Value::String(receiver.to_display_string()),
        Native::FunctionToString => Value::String(receiver.to_display_string()),
        // `Function` is here for its prototype and its `toString`. Building a function from a
        // string needs the compiler at run time, which is a different kind of realm from the one
        // an ahead-of-time compiler targets -- and saying so is better than the `undefined` this
        // used to return, which turned up three frames later as "undefined is not a function".
        Native::FunctionConstructor if !arguments.is_empty() => {
            return Err(ExecutionError::FunctionFromSource);
        }
        Native::FunctionConstructor => Value::Undefined,
        // `Proxy()` without `new` is a TypeError in the specification; there is nothing useful
        // to return either way.
        Native::ProxyConstructor => Value::Undefined,
        Native::ProxyRevocable => {
            let proxy = Value::Proxy(Rc::new(ProxyData {
                target: arguments.first().cloned().unwrap_or(Value::Undefined),
                handler: arguments.get(1).cloned().unwrap_or(Value::Undefined),
                revoked: std::cell::Cell::new(false),
            }));
            let mut pair = ObjectData::default();
            // The revoke function carries the proxy as its receiver, which is what a bound
            // native is for: there is no other way to give a built-in a captured value.
            pair.set(
                "revoke",
                Value::Bound(Rc::new(BoundFunction {
                    target: Value::Native(Native::ProxyRevoke),
                    this_value: proxy.clone(),
                    arguments: Vec::new(),
                })),
            );
            pair.set("proxy", proxy);
            Value::Object(Rc::new(RefCell::new(pair)))
        }
        Native::ProxyRevoke => {
            if let Value::Proxy(proxy) = receiver {
                proxy.revoked.set(true);
            }
            Value::Undefined
        }
        // A method table is not itself callable, and nothing reaches here by calling one.
        Native::PrototypeOf(_) => Value::Undefined,
        // `Object.is` is SameValue: `===` except that NaN equals itself and the two zeroes do
        // not equal each other, which are exactly the two cases `===` gets "wrong" on purpose.
        Native::ObjectIs => {
            let left = arguments.first().unwrap_or(&Value::Undefined);
            let right = arguments.get(1).unwrap_or(&Value::Undefined);
            Value::Boolean(match (left, right) {
                (Value::Number(a), Value::Number(b)) => {
                    if a.is_nan() && b.is_nan() {
                        true
                    } else {
                        a == b && a.is_sign_negative() == b.is_sign_negative()
                    }
                }
                _ => strict_equal(left, right),
            })
        }
        Native::ArrayFlat => match receiver {
            Value::Array(items) => {
                // One level unless told otherwise, which is the default the language chose.
                let depth = match arguments.first().map(Value::as_number) {
                    Some(depth) if depth.is_finite() => depth.max(0.0) as usize,
                    Some(_) => usize::MAX,
                    None => 1,
                };
                Value::Array(ArrayData::new(flatten(&items.borrow(), depth)))
            }
            _ => Value::Undefined,
        },
        Native::ArrayEntries | Native::ArrayKeys => match receiver {
            Value::Array(items) => {
                let entries = items
                    .borrow()
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let index = Value::Number(index as f64);
                        if matches!(native, Native::ArrayKeys) {
                            index
                        } else {
                            Value::Array(ArrayData::new(vec![
                                index,
                                if matches!(item, Value::Native(Native::Hole)) {
                                    Value::Undefined
                                } else {
                                    item.clone()
                                },
                            ]))
                        }
                    })
                    .collect();
                // The snapshot is walked by the ordinary array iterator, so `entries()` and
                // `values()` share one `next`.
                call_native(
                    Native::ArrayValues,
                    &Value::Array(ArrayData::new(entries)),
                    &[],
                )?
            }
            _ => Value::Undefined,
        },
        Native::ArrayOf => Value::Array(ArrayData::new(arguments.to_vec())),
        Native::StringFromCharCode => Value::String(String::from_utf16_lossy(
            &arguments
                .iter()
                .map(|value| value.as_number() as u16)
                .collect::<Vec<_>>(),
        )),
        Native::ObjectFreeze => {
            match arguments.first().unwrap_or(&Value::Undefined) {
                Value::Object(object) => object.borrow_mut().freeze(),
                // An array's *elements* are not properties of its property store, so freezing
                // has to reach them separately. Without this, `Object.freeze(list)` looked like
                // it worked and changed nothing at all.
                Value::Array(items) => items.properties.borrow_mut().freeze(),
                Value::Function(closure) => closure.properties.borrow_mut().freeze(),
                _ => {}
            }
            // Returns its argument, which is what makes `const o = Object.freeze({…})` read well.
            arguments.first().cloned().unwrap_or(Value::Undefined)
        }
        Native::ObjectIsFrozen => Value::Boolean(match arguments.first() {
            Some(Value::Object(object)) => object.borrow().is_frozen(),
            Some(Value::Array(items)) => items.properties.borrow().is_frozen(),
            Some(Value::Function(closure)) => closure.properties.borrow().is_frozen(),
            // A primitive cannot be changed at all, so it is frozen by definition.
            _ => true,
        }),
        Native::ObjectHasOwn => Value::Boolean(
            arguments
                .first()
                .is_some_and(|target| own_property_exists(
                    target,
                    arguments.get(1).unwrap_or(&Value::Undefined),
                )),
        ),
        Native::ObjectCreate => {
            let mut data = ObjectData::default();
            match arguments.first() {
                // `Object.create(null)` records the absence, which is not the same as recording
                // nothing: an object with no link at all inherits `Object.prototype`.
                Some(Value::Null) => data.set_prototype(Value::Null),
                Some(Value::Undefined) | None => {}
                Some(prototype) => data.set_prototype(prototype.clone()),
            }
            Value::Object(Rc::new(RefCell::new(data)))
        }
        Native::ObjectPreventExtensions | Native::ObjectSeal => {
            let target = arguments.first().cloned().unwrap_or(Value::Undefined);
            match &target {
                Value::Object(object) => object.borrow_mut().prevent_extensions(),
                Value::Function(closure) => closure.properties.borrow_mut().prevent_extensions(),
                Value::Array(items) => items.properties.borrow_mut().prevent_extensions(),
                _ => {}
            }
            target
        }
        Native::ObjectIsExtensible | Native::ObjectIsSealed => {
            let extensible = match arguments.first() {
                Some(Value::Object(object)) => object.borrow().is_extensible(),
                Some(Value::Function(closure)) => closure.properties.borrow().is_extensible(),
                Some(Value::Array(items)) => items.properties.borrow().is_extensible(),
                // A primitive is neither extensible nor unsealed, and both questions answer the
                // same way for it.
                _ => false,
            };
            Value::Boolean(if matches!(native, Native::ObjectIsSealed) {
                !extensible
            } else {
                extensible
            })
        }
        Native::ObjectGetOwnPropertySymbols => Value::Array(ArrayData::new(
            own_property_keys(arguments.first().unwrap_or(&Value::Undefined))
                .into_iter()
                .filter_map(|key| match key { PropertyKey::Symbol(symbol) => Some(Value::Symbol(symbol)), _ => None })
                .collect(),
        )),
        Native::ObjectGetOwnPropertyDescriptor => {
            let target = arguments.first().unwrap_or(&Value::Undefined);
            let key = arguments.get(1).map_or_else(
                || PropertyKey::String(String::new()),
                Value::to_property_key,
            );
            own_descriptor(target, &key).unwrap_or(Value::Undefined)
        }
        Native::ObjectGetOwnPropertyDescriptors => {
            let target = arguments.first().unwrap_or(&Value::Undefined);
            let mut data = ObjectData::default();
            for key in own_property_keys(target) {
                if let Some(descriptor) = own_descriptor(target, &key) {
                    data.set(&key, descriptor);
                }
            }
            Value::Object(Rc::new(RefCell::new(data)))
        }
        Native::Reflect => Value::Undefined,
        Native::ObjectOwnNames => {
            // Every own property, enumerated or not. A function's own properties are included
            // for a reason that took a while to find: the CommonJS interop every bundler emits
            // copies a module's exports with this, and a module whose exports *are* a function
            // -- lodash, and most of the older ecosystem -- copied nothing without it.
            Value::Array(ArrayData::new(
                own_property_keys(arguments.first().unwrap_or(&Value::Undefined))
                    .into_iter()
                    .filter_map(|key| string_key(&key).map(str::to_owned).map(Value::String))
                    .collect(),
            ))
        }
        Native::ObjectKeys => {
            let keys = own_properties(arguments.first().unwrap_or(&Value::Undefined))
                .into_iter()
                .filter_map(|(key, _)| string_key(&key).map(str::to_owned).map(Value::String))
                .collect();
            Value::Array(ArrayData::new(keys))
        }
        // Converting an object means calling its `toString`, which needs the machine.
        Native::StringConstructor => {
            unreachable!("`String` is dispatched by the interpreter")
        }
        Native::ErrorToString => Value::String(error_to_string(receiver)),
        Native::RegExpToString => Value::String(receiver.to_display_string()),
        // `new ArrayBuffer(n)` is n zero bytes. Called without `new` it is a TypeError in the
        // specification; there is nothing more useful to return either way.
        Native::ArrayBufferConstructor => {
            let length = arguments.first().map_or(0.0, Value::as_number);
            let length = if length.is_finite() && length >= 0.0 {
                length as usize
            } else {
                0
            };
            Value::ArrayBuffer(Rc::new(RefCell::new(vec![0; length])))
        }
        Native::TypedArrayConstructor(kind) => construct_typed_array(kind, arguments),
        // `of` takes its elements as arguments where the constructor takes one list, which is
        // the whole difference: `Uint8Array.of(3)` is one element, `new Uint8Array(3)` is three.
        Native::TypedArrayOf(kind) => {
            construct_typed_array(kind, &[Value::Array(ArrayData::new(arguments.to_vec()))])
        }
        // `view.set(source, offset?)` copies elements in, converting each as an assignment does.
        Native::TypedArraySet => {
            let Value::TypedArray(view) = receiver else {
                return Ok(Value::Undefined);
            };
            let offset = arguments.get(1).map_or(0.0, Value::as_number);
            let offset = if offset.is_finite() && offset >= 0.0 {
                offset as usize
            } else {
                0
            };
            for (index, item) in typed_source(arguments.first()).into_iter().enumerate() {
                view.set(offset + index, item);
            }
            Value::Undefined
        }
        // `subarray` shares the buffer; `slice` copies it. That difference is the whole reason
        // both exist, and getting it backwards would make a write visible where it should not be.
        Native::TypedArraySubarray => {
            let Value::TypedArray(view) = receiver else {
                return Ok(Value::Undefined);
            };
            let (start, end) = slice_bounds(arguments, view.length);
            Value::TypedArray(Rc::new(TypedArrayData {
                kind: view.kind,
                buffer: Rc::clone(&view.buffer),
                offset: view.offset + start * view.kind.width(),
                length: end.saturating_sub(start),
            }))
        }
        Native::TypedArrayFill => {
            let Value::TypedArray(view) = receiver else {
                return Ok(Value::Undefined);
            };
            let filler = arguments.first().map_or(0.0, Value::as_number);
            let (start, end) = slice_bounds(arguments.get(1..).unwrap_or_default(), view.length);
            for index in start..end {
                view.set(index, filler);
            }
            receiver.clone()
        }
        // `escape` and `unescape` are the pre-Unicode pair the specification keeps in Annex B
        // and marks as deprecated. They are here because real code still reaches for them: the
        // `unescape(encodeURIComponent(s))` idiom is how a library gets a string's UTF-8 bytes
        // one per code unit, and uuid does exactly that.
        Native::Escape => {
            let text = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let mut out = String::with_capacity(text.len());
            for unit in text.encode_utf16() {
                match char::from_u32(u32::from(unit)) {
                    Some(character)
                        if character.is_ascii_alphanumeric()
                            || matches!(character, '@' | '*' | '_' | '+' | '-' | '.' | '/') =>
                    {
                        out.push(character);
                    }
                    // Two hex digits below 256, four with a `%u` prefix above it. That split is
                    // the whole reason `escape` was replaced.
                    _ if unit < 256 => out.push_str(&format!("%{unit:02X}")),
                    _ => out.push_str(&format!("%u{unit:04X}")),
                }
            }
            Value::String(out)
        }
        Native::Unescape => {
            let text = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let units: Vec<u16> = text.encode_utf16().collect();
            let mut out: Vec<u16> = Vec::with_capacity(units.len());
            let mut index = 0;
            let hex = |units: &[u16], at: usize, count: usize| -> Option<u16> {
                let digits: String = units
                    .get(at..at + count)?
                    .iter()
                    .map(|unit| char::from_u32(u32::from(*unit)).unwrap_or('\0'))
                    .collect();
                u16::from_str_radix(&digits, 16).ok()
            };
            while index < units.len() {
                if units[index] != u16::from(b'%') {
                    out.push(units[index]);
                    index += 1;
                    continue;
                }
                if units.get(index + 1) == Some(&u16::from(b'u'))
                    && let Some(unit) = hex(&units, index + 2, 4)
                {
                    out.push(unit);
                    index += 6;
                    continue;
                }
                if let Some(unit) = hex(&units, index + 1, 2) {
                    out.push(unit);
                    index += 3;
                    continue;
                }
                // A `%` that does not introduce an escape stands for itself, unlike `decodeURI`
                // where the same text is an error.
                out.push(units[index]);
                index += 1;
            }
            Value::String(String::from_utf16_lossy(&out))
        }
        // The four URI functions differ only in which characters they leave alone. `encodeURI`
        // keeps the ones that structure a URI -- `:/?#[]@!$&'()*+,;=` -- because it is encoding a
        // whole one; `encodeURIComponent` encodes those too, because a component that contains a
        // `/` must not look like a path separator.
        Native::EncodeUri | Native::EncodeUriComponent => {
            let text = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let whole = matches!(native, Native::EncodeUri);
            let mut out = String::with_capacity(text.len());
            for character in text.chars() {
                let unreserved = character.is_ascii_alphanumeric()
                    || matches!(character, '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')');
                let reserved = matches!(
                    character,
                    ';' | '/' | '?' | ':' | '@' | '&' | '=' | '+' | '$' | ',' | '#'
                );
                if unreserved || (whole && reserved) {
                    out.push(character);
                    continue;
                }
                let mut buffer = [0_u8; 4];
                for byte in character.encode_utf8(&mut buffer).as_bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
            Value::String(out)
        }
        Native::DecodeUri | Native::DecodeUriComponent => {
            let text = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let bytes = text.as_bytes();
            let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
            let mut index = 0;
            while index < bytes.len() {
                if bytes[index] != b'%' {
                    out.push(bytes[index]);
                    index += 1;
                    continue;
                }
                // A `%` not followed by two hex digits is a URIError, not a literal percent.
                let Some(pair) = text.get(index + 1..index + 3) else {
                    return Err(ExecutionError::MalformedUri);
                };
                let Ok(byte) = u8::from_str_radix(pair, 16) else {
                    return Err(ExecutionError::MalformedUri);
                };
                // `decodeURI` leaves the reserved set encoded: undoing it would change what the
                // URI means, which is the one thing decoding must not do.
                if matches!(native, Native::DecodeUri)
                    && matches!(
                        byte,
                        b';' | b'/' | b'?' | b':' | b'@' | b'&' | b'=' | b'+' | b'$' | b',' | b'#'
                    )
                {
                    out.extend_from_slice(&bytes[index..index + 3]);
                } else {
                    out.push(byte);
                }
                index += 3;
            }
            match String::from_utf8(out) {
                Ok(decoded) => Value::String(decoded),
                Err(_) => return Err(ExecutionError::MalformedUri),
            }
        }
        Native::ObjectSetPrototypeOf => {
            let target = arguments.first().cloned().unwrap_or(Value::Undefined);
            if let Some(prototype) = arguments.get(1) {
                match &target {
                    Value::Object(object) => object.borrow_mut().set_prototype(prototype.clone()),
                    // A function is an object, and setting its prototype is how a program gives
                    // a constructor inherited statics without `extends`.
                    Value::Function(closure) => closure
                        .properties
                        .borrow_mut()
                        .set_prototype(prototype.clone()),
                    // Where `prototype_of` already looks for it, so the two agree.
                    Value::Array(items) => items
                        .properties
                        .borrow_mut()
                        .set_prototype(prototype.clone()),
                    _ => {}
                }
            }
            target
        }
        Native::ObjectGetPrototypeOf => {
            prototype_of(arguments.first().unwrap_or(&Value::Undefined))
        }
        // Both read the clock, which lives on the machine and is virtual -- see ADR 0015.
        Native::SymbolFor | Native::SymbolKeyFor => {
            unreachable!("the symbol registry is dispatched by the interpreter")
        }
        // Each of these *reads* a property, which for an accessor is a call, or calls back into
        // the program outright.
        Native::ObjectAssign
        | Native::ObjectEntries
        | Native::ObjectValues
        | Native::ObjectDefineProperty
        | Native::ObjectDefineProperties
        | Native::StringRaw
        | Native::CollectionForEach
        | Native::FunctionCall
        | Native::FunctionApply
        | Native::FunctionBind
        | Native::ObjectFromEntries
        | Native::ReflectOwnKeys
        | Native::ReflectGet
        | Native::ReflectSet
        | Native::ReflectApply => {
            unreachable!("these built-ins are dispatched by the interpreter")
        }
        Native::DateConstructor | Native::DateNow | Native::PerformanceNow => {
            unreachable!("the clock built-ins are dispatched by the interpreter")
        }
        // Namespaces and constructors, none of which is a plain call.
        Native::Performance
        | Native::WeakMapConstructor
        | Native::WeakSetConstructor => Value::Undefined,
        // A regexp operation reads and writes `lastIndex`, builds result arrays, and may call a
        // replacement function -- so none of them is a pure function of its arguments either.
        Native::RegExpConstructor
        | Native::RegExpTest
        | Native::RegExpExec
        | Native::StringMatch
        | Native::StringMatchAll
        | Native::StringSearch => {
            unreachable!("the regexp built-ins are dispatched by the interpreter")
        }
        // Peeled off by `dispatch_native`, which every call site goes through: scheduling and
        // walking an iterable both need the interpreter, and this function deliberately does
        // not have one.
        Native::SetTimeout
        | Native::SetInterval
        | Native::ClearTimeout
        | Native::ClearInterval
        | Native::QueueMicrotask
        // ...and `Math.random` advances state that lives on the machine.
        | Native::MathRandom
        // Every promise operation settles something, schedules something, or calls back into the
        // program -- so none of them is a pure function of its arguments.
        | Native::PromiseConstructor
        | Native::PromiseResolve
        | Native::PromiseReject
        | Native::PromiseAll
        | Native::PromiseAllSettled
        | Native::PromiseRace
        | Native::PromiseAny
        | Native::PromiseThen
        | Native::PromiseCatch
        | Native::PromiseFinally => {
            unreachable!("the scheduling built-ins are dispatched by the interpreter")
        }
        // A marker, not a callable: every read of it is turned into a ReferenceError before it
        // can reach an expression, so calling one means a read slipped through.
        Native::Uninitialized => unreachable!("the dead-zone marker is never a value"),
        // Reading a hole yields undefined, so the marker never reaches an expression either.
        Native::Hole => unreachable!("an array hole is never a value"),
        // Peeled off the same way, for the same reason.
        Native::ArrayFrom | Native::TypedArrayFrom(_) => {
            unreachable!("`from` is dispatched by the interpreter")
        }
        Native::ArrayPush => match receiver {
            Value::Array(items) => {
                let mut items = items.borrow_mut();
                items.extend(arguments.iter().cloned());
                Value::Number(items.len() as f64)
            }
            _ => Value::Undefined,
        },
        Native::ArrayPop => match receiver {
            // Popping an empty array is `undefined`, not an error.
            Value::Array(items) => {
                let mut values = items.borrow_mut();
                let index = values.len().checked_sub(1);
                let result = values.pop().unwrap_or(Value::Undefined);
                drop(values);
                if let Some(index) = index {
                    items.remove_index_attributes(index);
                }
                result
            }
            _ => Value::Undefined,
        },
        Native::ArrayIndexOf | Native::ArrayIncludes => {
            let needle = arguments.first().cloned().unwrap_or(Value::Undefined);
            let includes = matches!(native, Native::ArrayIncludes);
            let found = match receiver {
                Value::Array(items) => items.borrow().iter().position(|item| match item {
                    // A hole is skipped by `indexOf` and *seen* by `includes`, which reads it as
                    // the undefined it behaves like everywhere else. The two disagree here and
                    // nowhere else, which is the whole difference between strict equality and
                    // SameValueZero plus this one rule.
                    Value::Native(Native::Hole) => {
                        includes && matches!(needle, Value::Undefined)
                    }
                    // `indexOf` uses strict equality, so NaN is never found and `-0` matches
                    // `0`; `includes` uses SameValueZero, where NaN finds itself.
                    item if includes
                        && matches!((item, &needle), (Value::Number(a), Value::Number(b))
                            if a.is_nan() && b.is_nan()) =>
                    {
                        true
                    }
                    item => strict_equal(item, &needle),
                }),
                _ => None,
            };
            if matches!(native, Native::ArrayIncludes) {
                Value::Boolean(found.is_some())
            } else {
                Value::Number(found.map_or(-1.0, |index| index as f64))
            }
        }
        Native::ArraySlice => match receiver {
            Value::Array(items) => {
                let items = items.borrow();
                let start = relative_index(arguments.first(), items.len(), 0);
                let end = relative_index(arguments.get(1), items.len(), items.len());
                // `slice(3, 1)` is empty rather than an error, so the range is clamped both ways.
                let taken = items
                    .get(start..end.max(start))
                    .unwrap_or_default()
                    .to_vec();
                Value::Array(ArrayData::new(taken))
            }
            _ => Value::Undefined,
        },
        // Every reader takes the same decomposition apart, so they share one arm.
        // `setFullYear(y, m, d)` and its siblings take the fields after theirs too, which is why
        // this replaces a *run* of parts rather than one. The date mutates in place and answers
        // with its new instant, as the specification says.
        Native::DateSet(field) => {
            let Value::Date(time) = receiver else {
                return Ok(Value::Undefined);
            };
            if matches!(field, DateField::Time) {
                let updated = arguments.first().map_or(f64::NAN, Value::as_number);
                time.set(updated);
                return Ok(Value::Number(updated));
            }
            let Some(parts) = date_parts(time.get()) else {
                // Every setter but `setTime` on an invalid date leaves it invalid.
                return Ok(Value::Number(f64::NAN));
            };
            // The order the fields appear in, so "this one and the ones after it" is a slice.
            let mut values = [
                parts.year as f64,
                f64::from(parts.month) - 1.0,
                f64::from(parts.day),
                f64::from(parts.hour),
                f64::from(parts.minute),
                f64::from(parts.second),
                f64::from(parts.millisecond),
            ];
            let first = match field {
                DateField::FullYear => 0,
                DateField::Month => 1,
                DateField::Day => 2,
                DateField::Hours => 3,
                DateField::Minutes => 4,
                DateField::Seconds => 5,
                DateField::Milliseconds => 6,
                DateField::Time | DateField::Weekday | DateField::TimezoneOffset => {
                    return Ok(Value::Number(time.get()));
                }
            };
            for (offset, argument) in arguments.iter().enumerate() {
                let Some(slot) = values.get_mut(first + offset) else {
                    break;
                };
                *slot = argument.as_number();
            }
            let updated = parts_to_instant(values);
            time.set(updated);
            Value::Number(updated)
        }
        Native::DateField(field) => {
            let Value::Date(time) = receiver else {
                return Ok(Value::Undefined);
            };
            let time = time.get();
            if matches!(field, DateField::Time) {
                return Ok(Value::Number(time));
            }
            if matches!(field, DateField::TimezoneOffset) {
                return Ok(Value::Number(0.0));
            }
            let Some(parts) = date_parts(time) else {
                // Every reader on an invalid date is NaN, which is what makes one propagate.
                return Ok(Value::Number(f64::NAN));
            };
            Value::Number(match field {
                // `getMonth` is zero-based and `getDate` is one-based, which is the language's
                // oldest wart and is not this compiler's to fix.
                DateField::FullYear => parts.year as f64,
                DateField::Month => f64::from(parts.month) - 1.0,
                DateField::Day => f64::from(parts.day),
                DateField::Weekday => f64::from(parts.weekday),
                DateField::Hours => f64::from(parts.hour),
                DateField::Minutes => f64::from(parts.minute),
                DateField::Seconds => f64::from(parts.second),
                DateField::Milliseconds => f64::from(parts.millisecond),
                DateField::Time | DateField::TimezoneOffset => unreachable!("handled above"),
            })
        }
        Native::DateToIso => {
            let Value::Date(time) = receiver else {
                return Ok(Value::Undefined);
            };
            match iso_string(time.get()) {
                Some(text) => Value::String(text),
                // An instant with no ISO spelling is a RangeError, not a placeholder string.
                None => return Err(ExecutionError::RadixOutOfRange),
            }
        }
        Native::DateToJson => {
            let Value::Date(time) = receiver else {
                return Ok(Value::Undefined);
            };
            match iso_string(time.get()) {
                Some(text) => Value::String(text),
                None => Value::Null,
            }
        }
        Native::DateToString => {
            let Value::Date(time) = receiver else {
                return Ok(Value::Undefined);
            };
            Value::String(date_to_string(time.get()))
        }
        Native::DateParse => Value::Number(parse_date(
            &arguments
                .first()
                .map_or_else(String::new, Value::to_display_string),
        )),
        Native::DateUtc => Value::Number(date_from_fields(arguments)),
        Native::ArrayAt => match receiver {
            Value::Array(items) => {
                let items = items.borrow();
                let index = arguments.first().map_or(0.0, Value::as_number);
                let index = if index < 0.0 {
                    items.len() as f64 + index
                } else {
                    index
                };
                if index < 0.0 || index >= items.len() as f64 {
                    Value::Undefined
                } else {
                    items[index as usize].clone()
                }
            }
            _ => Value::Undefined,
        },
        // `concat` flattens an array argument one level and appends anything else as one
        // element, which is the one place an array argument is not just a value.
        Native::ArrayConcat => match receiver {
            Value::Array(items) => {
                let mut joined = items.borrow().clone();
                for argument in arguments {
                    match argument {
                        Value::Array(more) => joined.extend(more.borrow().iter().cloned()),
                        other => joined.push(other.clone()),
                    }
                }
                Value::Array(ArrayData::new(joined))
            }
            _ => Value::Undefined,
        },
        // In place, and returns the same array rather than a copy.
        Native::ArrayReverse => match receiver {
            Value::Array(items) => {
                let old_values = items.borrow().clone();
                let old_hidden = items.index_non_enumerable.borrow().clone();
                items.borrow_mut().reverse();
                let length = old_values.len();
                let hidden = (0..length).filter(|index| {
                    let other = length - 1 - *index;
                    old_values.get(*index).is_some_and(array_item_present)
                        && old_values.get(other).is_some_and(array_item_present)
                        && old_hidden.contains(index)
                });
                items.replace_index_attributes(hidden);
                receiver.clone()
            }
            _ => Value::Undefined,
        },
        Native::ArrayShift => match receiver {
            Value::Array(array) => {
                let old_values = array.borrow().clone();
                let old_hidden = array.index_non_enumerable.borrow().clone();
                let mut items = array.borrow_mut();
                if items.is_empty() {
                    Value::Undefined
                } else {
                    let result = items.remove(0);
                    let new_length = items.len();
                    drop(items);
                    let hidden = (0..new_length).filter(|index| {
                        old_values.get(*index).is_some_and(array_item_present)
                            && old_values
                                .get(*index + 1)
                                .is_some_and(array_item_present)
                            && old_hidden.contains(index)
                    });
                    array.replace_index_attributes(hidden);
                    result
                }
            }
            _ => Value::Undefined,
        },
        Native::ArrayUnshift => match receiver {
            Value::Array(array) => {
                let old_values = array.borrow().clone();
                let old_hidden = array.index_non_enumerable.borrow().clone();
                let mut items = array.borrow_mut();
                // Inserted as a block so the arguments keep their order at the front.
                items.splice(0..0, arguments.iter().cloned());
                let length = items.len();
                drop(items);
                let inserted = arguments.len();
                let hidden = (0..length).filter(|index| {
                    let source_present = if *index < inserted {
                        true
                    } else {
                        let source = index - inserted;
                        old_values.get(source).is_some_and(array_item_present)
                    };
                    old_values.get(*index).is_some_and(array_item_present)
                        && source_present
                        && old_hidden.contains(index)
                });
                array.replace_index_attributes(hidden);
                Value::Number(length as f64)
            }
            _ => Value::Undefined,
        },
        Native::ArrayLastIndexOf => match receiver {
            Value::Array(items) => {
                let needle = arguments.first().cloned().unwrap_or(Value::Undefined);
                Value::Number(
                    items
                        .borrow()
                        .iter()
                        .rposition(|item| strict_equal(item, &needle))
                        .map_or(-1.0, |index| index as f64),
                )
            }
            _ => Value::Undefined,
        },
        Native::ArrayJoinDefault => match receiver {
            Value::Array(items) => Value::String(join(&items.borrow(), ",")),
            _ => Value::String(String::new()),
        },
        Native::ArraySplice => match receiver {
            Value::Array(items) => {
                let old_values = items.borrow().clone();
                let old_hidden = items.index_non_enumerable.borrow().clone();
                let length = items.borrow().len();
                let start = relative_index(arguments.first(), length, 0);
                // A missing count removes everything from `start`, where an explicit `undefined`
                // removes nothing: `splice(1)` and `splice(1, undefined)` are different calls.
                let removing = match arguments.get(1) {
                    None => length - start,
                    Some(value) => {
                        let wanted = value.as_number();
                        if wanted.is_nan() || wanted < 0.0 {
                            0
                        } else {
                            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                            let wanted = wanted as usize;
                            wanted.min(length - start)
                        }
                    }
                };
                let inserted: Vec<Value> = arguments.get(2..).unwrap_or_default().to_vec();
                let removed: Vec<Value> = items
                    .borrow_mut()
                    .splice(start..start + removing, inserted)
                    .collect();
                let inserting = arguments.len().saturating_sub(2);
                let new_length = old_values.len() - removing + inserting;
                let hidden = (0..new_length).filter(|index| {
                    let source_present = if *index < start {
                        old_values.get(*index).is_some_and(array_item_present)
                    } else if *index < start + inserting {
                        true
                    } else {
                        let source = *index + removing - inserting;
                        old_values.get(source).is_some_and(array_item_present)
                    };
                    old_values.get(*index).is_some_and(array_item_present)
                        && source_present
                        && old_hidden.contains(index)
                });
                items.replace_index_attributes(hidden);
                // The removed elements, which is what makes `splice` usable as a queue read.
                Value::Array(ArrayData::new(removed))
            }
            _ => Value::Undefined,
        },
        Native::ArrayCopyWithin => match receiver {
            Value::Array(items) => {
                let old_values = items.borrow().clone();
                let old_hidden = items.index_non_enumerable.borrow().clone();
                let length = items.borrow().len();
                let target = relative_index(arguments.first(), length, 0);
                let start = relative_index(arguments.get(1), length, 0);
                let end = relative_index(arguments.get(2), length, length);
                // Copied out first, because the source and the destination overlap by design --
                // that overlap is the whole reason the method exists.
                let taken: Vec<Value> = items
                    .borrow()
                    .iter()
                    .take(end)
                    .skip(start)
                    .take(length - target)
                    .cloned()
                    .collect();
                for (offset, value) in taken.into_iter().enumerate() {
                    if let Some(slot) = items.borrow_mut().get_mut(target + offset) {
                        *slot = value;
                    }
                }
                let copied = end.saturating_sub(start).min(length - target);
                let hidden = (0..length).filter(|index| {
                    if (target..target + copied).contains(index) {
                        let source = start + (*index - target);
                        old_values.get(*index).is_some_and(array_item_present)
                            && old_values.get(source).is_some_and(array_item_present)
                            && old_hidden.contains(index)
                    } else {
                        old_hidden.contains(index)
                    }
                });
                items.replace_index_attributes(hidden);
                receiver.clone()
            }
            _ => Value::Undefined,
        },
        Native::ArrayFill => match receiver {
            Value::Array(items) => {
                let old_values = items.borrow().clone();
                let old_hidden = items.index_non_enumerable.borrow().clone();
                let value = arguments.first().cloned().unwrap_or(Value::Undefined);
                let length = items.borrow().len();
                let start = relative_index(arguments.get(1), length, 0);
                let end = relative_index(arguments.get(2), length, length);
                for slot in items.borrow_mut().iter_mut().take(end).skip(start) {
                    *slot = value.clone();
                }
                let hidden = (0..length).filter(|index| {
                    old_hidden.contains(index)
                        && (old_values.get(*index).is_some_and(array_item_present)
                            || !(start..end).contains(index))
                });
                items.replace_index_attributes(hidden);
                receiver.clone()
            }
            _ => Value::Undefined,
        },
        Native::ArrayJoin => {
            let separator = match arguments.first() {
                None | Some(Value::Undefined) => ",".to_owned(),
                Some(value) => value.to_display_string(),
            };
            match receiver {
                Value::Array(items) => Value::String(join(&items.borrow(), &separator)),
                _ => Value::String(String::new()),
            }
        }
        // Every one of these is a plain transformation of its arguments, which is what keeps
        // them in this function rather than on the interpreter.
        Native::MathAbs => Value::Number(argument_number(arguments).abs()),
        Native::MathFloor => Value::Number(argument_number(arguments).floor()),
        Native::MathCeil => Value::Number(argument_number(arguments).ceil()),
        // Not `f64::round`, which rounds a half away from zero: JavaScript rounds a half toward
        // positive infinity, so `Math.round(-1.5)` is -1 rather than -2.
        Native::MathRound => {
            let value = argument_number(arguments);
            Value::Number(if !value.is_finite() {
                value
            } else {
                let rounded = (value + 0.5).floor();
                // A negative input that rounds to zero gives *negative* zero, which the floor
                // has already thrown away. Only `Object.is` and `1 / x` can see the difference,
                // and a console shows it, so it is worth putting back.
                if rounded == 0.0 && value < 0.0 {
                    -0.0
                } else {
                    rounded
                }
            })
        }
        Native::MathTrunc => Value::Number(argument_number(arguments).trunc()),
        // `Math.sign` preserves the sign of zero and passes NaN through, which `signum` does not.
        Native::MathSign => {
            let value = argument_number(arguments);
            Value::Number(if value.is_nan() || value == 0.0 {
                value
            } else {
                value.signum()
            })
        }
        Native::MathSqrt => Value::Number(argument_number(arguments).sqrt()),
        Native::MathCbrt => Value::Number(argument_number(arguments).cbrt()),
        Native::MathSinh => Value::Number(argument_number(arguments).sinh()),
        Native::MathCosh => Value::Number(argument_number(arguments).cosh()),
        Native::MathTanh => Value::Number(argument_number(arguments).tanh()),
        Native::MathAsinh => Value::Number(argument_number(arguments).asinh()),
        Native::MathAcosh => Value::Number(argument_number(arguments).acosh()),
        Native::MathAtanh => Value::Number(argument_number(arguments).atanh()),
        // `ln(1 + x)` and `e^x - 1` as one operation each, which is the whole point of both: the
        // two-step spelling loses every significant digit for small `x`.
        Native::MathLog1p => Value::Number(argument_number(arguments).ln_1p()),
        Native::MathExpm1 => Value::Number(argument_number(arguments).exp_m1()),
        // The nearest `f32`, widened back. A round trip, not a truncation.
        #[expect(clippy::cast_possible_truncation)]
        Native::MathFround => Value::Number(f64::from(argument_number(arguments) as f32)),
        Native::MathClz32 => Value::Number(f64::from(to_uint32(argument_number(arguments)).leading_zeros())),
        // Both operands are truncated to 32-bit *first*, which is what makes this different from
        // `a * b`: the product is exact in 32 bits and wraps rather than rounding.
        Native::MathImul => Value::Number(f64::from(
            to_int32(argument_number(arguments))
                .wrapping_mul(to_int32(arguments.get(1).map_or(f64::NAN, Value::as_number))),
        )),
        // `hypot` is not `sqrt(x*x + y*y)`: squaring overflows for arguments whose result is
        // perfectly representable, which is the whole reason it exists.
        Native::MathHypot => Value::Number(arguments.iter().fold(0.0f64, |total, argument| {
            total.hypot(argument.as_number())
        })),
        Native::MathAtan2 => Value::Number(
            argument_number(arguments).atan2(arguments.get(1).map_or(f64::NAN, Value::as_number)),
        ),
        Native::MathSin => Value::Number(argument_number(arguments).sin()),
        Native::MathCos => Value::Number(argument_number(arguments).cos()),
        Native::MathTan => Value::Number(argument_number(arguments).tan()),
        Native::MathAsin => Value::Number(argument_number(arguments).asin()),
        Native::MathAcos => Value::Number(argument_number(arguments).acos()),
        Native::MathAtan => Value::Number(argument_number(arguments).atan()),
        Native::MathPow => Value::Number(
            argument_number(arguments).powf(arguments.get(1).map_or(f64::NAN, Value::as_number)),
        ),
        // `Math.min()` with no arguments is Infinity and `Math.max()` is -Infinity, which are the
        // identities for the fold rather than an error. Any NaN argument wins outright.
        Native::MathMin | Native::MathMax => {
            let smallest = matches!(native, Native::MathMin);
            let identity = if smallest {
                f64::INFINITY
            } else {
                f64::NEG_INFINITY
            };
            let mut best = identity;
            for argument in arguments {
                let value = argument.as_number();
                if value.is_nan() {
                    best = f64::NAN;
                    break;
                }
                if (smallest && value < best) || (!smallest && value > best) {
                    best = value;
                }
            }
            Value::Number(best)
        }
        Native::MathLog => Value::Number(argument_number(arguments).ln()),
        Native::MathLog2 => Value::Number(argument_number(arguments).log2()),
        Native::MathLog10 => Value::Number(argument_number(arguments).log10()),
        Native::MathExp => Value::Number(argument_number(arguments).exp()),
        // `Number(x)` converts; `Number()` with no argument is 0, not NaN.
        Native::NumberValueOf => Value::Number(receiver.as_number()),
        Native::NumberToString => {
            let value = receiver.as_number();
            match arguments.first().map(Value::as_number) {
                // Radix 10 is the default and is exactly `String(n)`, exponent rules included.
                None | Some(10.0) => Value::String(number_string(value)),
                Some(radix) if (2.0..=36.0).contains(&radix) => {
                    Value::String(number_in_radix(value, radix as u32))
                }
                // A radix outside 2..=36 is a RangeError; returning NaN would look like a result.
                _ => return Err(ExecutionError::RadixOutOfRange),
            }
        }
        Native::NumberToFixed => {
            let value = receiver.as_number();
            let digits = arguments.first().map_or(0.0, Value::as_number);
            if !(0.0..=100.0).contains(&digits) {
                return Err(ExecutionError::RadixOutOfRange);
            }
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Value::String(number_to_fixed(value, digits as u32))
        }
        Native::NumberConstructor => Value::Number(match arguments.first() {
            None => 0.0,
            Some(value) => value.as_number(),
        }),
        Native::BooleanConstructor => {
            Value::Boolean(arguments.first().is_some_and(Value::is_truthy))
        }
        // The `Number.*` predicates do not coerce: they answer about the argument as given, which
        // is the whole reason they exist alongside the global `isNaN` and `isFinite`.
        Native::NumberIsNaN => Value::Boolean(
            matches!(arguments.first(), Some(Value::Number(value)) if value.is_nan()),
        ),
        Native::NumberIsFinite => Value::Boolean(
            matches!(arguments.first(), Some(Value::Number(value)) if value.is_finite()),
        ),
        Native::NumberIsInteger => Value::Boolean(
            matches!(arguments.first(), Some(Value::Number(value)) if is_integer(*value)),
        ),
        Native::NumberIsSafeInteger => {
            Value::Boolean(matches!(arguments.first(), Some(Value::Number(value))
                if is_integer(*value) && value.abs() <= 9_007_199_254_740_991.0))
        }
        // The globals do coerce, which is the difference.
        Native::IsNaN => Value::Boolean(argument_number(arguments).is_nan()),
        Native::IsFinite => Value::Boolean(argument_number(arguments).is_finite()),
        Native::ParseInt => parse_int(arguments),
        Native::ParseFloat => parse_float(arguments),
        // Serialising reads every property, and reading an accessor is a call.
        Native::JsonStringify => {
            unreachable!("`JSON.stringify` is dispatched by the interpreter")
        }
        Native::JsonParse => {
            let text = match arguments.first() {
                None | Some(Value::Undefined) => "undefined".to_owned(),
                Some(value) => value.to_display_string(),
            };
            let mut reader = JsonReader {
                rest: text.as_str(),
            };
            reader.skip_whitespace();
            let value = reader.value()?;
            reader.skip_whitespace();
            if !reader.rest.is_empty() {
                return Err(ExecutionError::InvalidJson);
            }
            value
        }
        // Present so `typeof` reports "function", which is what the corpus checks. Calling it
        // returns undefined rather than inventing a stack this VM does not record.
        Native::ErrorCaptureStackTrace => Value::Undefined,
        // `Array.prototype.values` should return an iterator object. Nothing consumes one yet
        // -- for-of is a later step -- and the corpus only checks the function's identity and
        // type, so returning undefined is the honest placeholder rather than a fake iterator
        // that would break the moment anything called `.next()` on it.
        // A real iterator object: state lives in reserved properties on the iterator itself,
        // so it needs no special value kind and `next` is an ordinary method call.
        Native::ArrayValues => match receiver {
            Value::Array(_) => {
                let mut iterator = ObjectData::default();
                iterator.set(internal_key("iterated"), receiver.clone());
                iterator.set(internal_key("index"), Value::Number(0.0));
                iterator.set("next", Value::Native(Native::ArrayIteratorNext));
                // An iterator is itself iterable, which is what lets one be passed straight to
                // another `for-of` or to `Array.from`.
                iterator.set(
                    iterator_key(),
                    Value::Native(Native::IteratorSelf),
                );
                Value::Object(Rc::new(RefCell::new(iterator)))
            }
            _ => Value::Undefined,
        },
        Native::IteratorSelf => receiver.clone(),
        Native::StringToUpperCase
        | Native::StringToLowerCase
        | Native::StringTrim
        | Native::StringIncludes
        | Native::StringIndexOf
        | Native::StringSlice
        | Native::StringSplit
        | Native::StringStartsWith
        | Native::StringEndsWith
        | Native::StringRepeat
        | Native::StringAt
        | Native::StringCharAt
        | Native::StringCharCodeAt
        | Native::StringPadStart
        | Native::StringPadEnd
        | Native::StringReplace
        | Native::StringReplaceAll
        | Native::StringLastIndexOf
        | Native::StringTrimStart
        | Native::StringTrimEnd
        | Native::StringConcat
        | Native::StringSubstring
        | Native::StringSubstr
        | Native::StringCodePointAt
        | Native::StringLocaleCompare
        | Native::StringNormalize => {
            let Value::String(text) = receiver else {
                return Ok(Value::Undefined);
            };
            let argument = |index: usize| {
                arguments
                    .get(index)
                    .map_or_else(String::new, Value::to_display_string)
            };
            match native {
                Native::StringToUpperCase => Value::String(text.to_uppercase()),
                Native::StringToLowerCase => Value::String(text.to_lowercase()),
                Native::StringTrim => Value::String(text.trim().to_owned()),
                Native::StringIncludes => Value::Boolean(text.contains(&argument(0))),
                Native::StringStartsWith => Value::Boolean(text.starts_with(&argument(0))),
                Native::StringEndsWith => Value::Boolean(text.ends_with(&argument(0))),
                Native::StringIndexOf => Value::Number(
                    // Byte offsets are not character offsets; the index has to be counted in
                    // the same units `length` reports, which is UTF-16.
                    text.find(&argument(0))
                        .map_or(-1.0, |byte| text[..byte].encode_utf16().count() as f64),
                ),
                Native::StringRepeat => {
                    let count = arguments.first().map_or(0.0, Value::as_number);
                    if count < 0.0 || !count.is_finite() {
                        Value::String(String::new())
                    } else {
                        Value::String(text.repeat(count as usize))
                    }
                }
                Native::StringSplit => {
                    let separator = argument(0);
                    let parts: Vec<Value> = if arguments.is_empty() {
                        vec![Value::String(text.clone())]
                    } else if separator.is_empty() {
                        // An empty separator splits into characters, not into nothing.
                        text.chars().map(|c| Value::String(c.to_string())).collect()
                    } else {
                        text.split(&separator)
                            .map(|part| Value::String(part.to_owned()))
                            .collect()
                    };
                    Value::Array(ArrayData::new(parts))
                }
                // `at` accepts a negative index, where `charAt` clamps and `[i]` gives
                // undefined -- three spellings of "the character there" that differ only at the
                // edges, which is exactly where they get used.
                Native::StringAt => {
                    // Code units, like `charAt` and `[i]` and `length`: three spellings of "the
                    // character there" that differ only at the edges, and a fourth unit would
                    // make them differ in the middle as well.
                    let units: Vec<u16> = text.encode_utf16().collect();
                    let index = arguments.first().map_or(0.0, Value::as_number);
                    let index = if index < 0.0 {
                        units.len() as f64 + index
                    } else {
                        index
                    };
                    if index < 0.0 || index >= units.len() as f64 {
                        Value::Undefined
                    } else {
                        let index = index as usize;
                        Value::String(utf16_slice(&units, index..index + 1))
                    }
                }
                Native::StringCharAt => {
                    // Code units, not characters: `charAt` counts what `length` counts, so half
                    // of a surrogate pair is a position and reads as that half.
                    let units: Vec<u16> = text.encode_utf16().collect();
                    let index = arguments.first().map_or(0.0, Value::as_number);
                    // Out of range is the empty string, not undefined.
                    if index < 0.0 || index >= units.len() as f64 {
                        Value::String(String::new())
                    } else {
                        Value::String(String::from_utf16_lossy(&units[index as usize..=index as usize]))
                    }
                }
                Native::StringCharCodeAt => {
                    let units: Vec<u16> = text.encode_utf16().collect();
                    let index = arguments.first().map_or(0.0, Value::as_number);
                    // UTF-16 code units, so an astral character has two and this reads one.
                    if index < 0.0 || index >= units.len() as f64 {
                        Value::Number(f64::NAN)
                    } else {
                        Value::Number(f64::from(units[index as usize]))
                    }
                }
                Native::StringPadStart | Native::StringPadEnd => {
                    let target = arguments.first().map_or(0.0, Value::as_number);
                    let padding = match arguments.get(1) {
                        None | Some(Value::Undefined) => " ".to_owned(),
                        Some(value) => value.to_display_string(),
                    };
                    let current = text.encode_utf16().count();
                    let target = if target.is_finite() && target > 0.0 {
                        target as usize
                    } else {
                        0
                    };
                    // Padding with the empty string cannot make progress, so it is left alone
                    // rather than looping forever.
                    if target <= current || padding.is_empty() {
                        Value::String(text.clone())
                    } else {
                        let mut filler = String::new();
                        while filler.encode_utf16().count() < target - current {
                            filler.push_str(&padding);
                        }
                        // The last repetition is cut to length rather than overshooting.
                        let filler: String = filler
                            .chars()
                            .scan(0usize, |units, character| {
                                if *units >= target - current {
                                    return None;
                                }
                                *units += character.len_utf16();
                                Some(character)
                            })
                            .collect();
                        Value::String(if matches!(native, Native::StringPadStart) {
                            format!("{filler}{text}")
                        } else {
                            format!("{text}{filler}")
                        })
                    }
                }
                // String patterns only. A regular expression is a different search and a
                // different replacement grammar, and neither exists yet.
                Native::StringReplace | Native::StringReplaceAll => {
                    let pattern = arguments
                        .first()
                        .map_or_else(String::new, Value::to_display_string);
                    let replacement = arguments
                        .get(1)
                        .map_or_else(String::new, Value::to_display_string);
                    // The replacement is a *template*, not literal text, even when the pattern
                    // is a plain string: `$&` is what matched, `` $` `` and `$'` are what came
                    // before and after, and `$$` is one dollar. Treating it literally is the
                    // kind of wrong answer that only shows up in output nobody reads closely.
                    // The replacement is a *template*, not literal text, even when the pattern
                    // is a plain string: `$&` is what matched, `` $` `` and `$'` are what came
                    // before and after it, and `$$` is one dollar. The same expander the regexp
                    // path uses does it, so the two cannot disagree.
                    let all = matches!(native, Native::StringReplaceAll);
                    let subject: Vec<u16> = text.encode_utf16().collect();
                    let needle: Vec<u16> = pattern.encode_utf16().collect();
                    let mut out = String::with_capacity(text.len());
                    let mut at = 0;
                    loop {
                        // An empty pattern matches at every position including the end, which is
                        // why it cannot go through the window search.
                        let found = if needle.is_empty() {
                            (at <= subject.len()).then_some(at)
                        } else {
                            subject
                                .get(at..)
                                .and_then(|rest| {
                                    rest.windows(needle.len())
                                        .position(|window| window == needle.as_slice())
                                })
                                .map(|offset| at + offset)
                        };
                        let Some(start) = found else { break };
                        out.push_str(&utf16_slice(&subject, at..start));
                        out.push_str(&expand_replacement(
                            &replacement,
                            &subject,
                            &(start..start + needle.len()),
                            &[],
                            &[],
                        ));
                        at = start + needle.len();
                        if !all {
                            break;
                        }
                        if needle.is_empty() {
                            if start < subject.len() {
                                // Carry across the unit the empty match stepped over.
                                out.push_str(&utf16_slice(&subject, start..start + 1));
                                at = start + 1;
                            } else {
                                at = subject.len() + 1;
                                break;
                            }
                        }
                    }
                    if at <= subject.len() {
                        out.push_str(&utf16_slice(&subject, at..subject.len()));
                    }
                    Value::String(out)
                }
                Native::StringLastIndexOf => {
                    let needle = arguments
                        .first()
                        .map_or_else(String::new, Value::to_display_string);
                    // The index counts what `length` counts, which is UTF-16 code units --
                    // not bytes, and not characters either.
                    Value::Number(text.rfind(&needle).map_or(-1.0, |offset| {
                        text[..offset].encode_utf16().count() as f64
                    }))
                }
                // `substring` clamps both ends into range and swaps them if they are the wrong
                // way round, where `slice` counts a negative from the end and yields nothing
                // when they cross. The two look alike and agree on almost no edge case.
                Native::StringSubstring => {
                    let units: Vec<u16> = text.encode_utf16().collect();
                    let clamp = |value: f64| -> usize {
                        if value.is_nan() || value < 0.0 {
                            0
                        } else {
                            (value as usize).min(units.len())
                        }
                    };
                    let first = clamp(arguments.first().map_or(0.0, Value::as_number));
                    let second = match arguments.get(1) {
                        Some(Value::Undefined) | None => units.len(),
                        Some(value) => clamp(value.as_number()),
                    };
                    let (start, end) = (first.min(second), first.max(second));
                    Value::String(String::from_utf16_lossy(&units[start..end]))
                }
                Native::StringSubstr => {
                    let units: Vec<u16> = text.encode_utf16().collect();
                    let start = arguments.first().map_or(0.0, Value::as_number);
                    // A negative start counts from the end, which is the one thing `substr`
                    // does that `substring` does not.
                    let start = if start < 0.0 {
                        (units.len() as f64 + start).max(0.0) as usize
                    } else {
                        (start as usize).min(units.len())
                    };
                    let count = match arguments.get(1) {
                        Some(Value::Undefined) | None => units.len() - start,
                        Some(value) => {
                            let count = value.as_number();
                            if count.is_nan() || count < 0.0 {
                                0
                            } else {
                                (count as usize).min(units.len() - start)
                            }
                        }
                    };
                    Value::String(String::from_utf16_lossy(&units[start..start + count]))
                }
                Native::StringCodePointAt => {
                    let index = arguments.first().map_or(0.0, Value::as_number);
                    let units: Vec<u16> = text.encode_utf16().collect();
                    if index < 0.0 || index >= units.len() as f64 {
                        Value::Undefined
                    } else {
                        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        let at = index as usize;
                        let lead = units.get(at).copied().unwrap_or(0);
                        // A surrogate *pair* reads as the whole code point, which is the entire
                        // difference from `charCodeAt`. A lone surrogate reads as itself: the
                        // answer is a number, so there is no string here to be unable to hold one
                        // (ADR 0022). Decoding through a `String` answered U+FFFD instead, which
                        // is what the trailing half of an emoji became.
                        let paired = if (0xD800..=0xDBFF).contains(&lead) {
                            units
                                .get(at + 1)
                                .filter(|low| (0xDC00..=0xDFFF).contains(*low))
                                .map(|low| {
                                    0x10000
                                        + ((u32::from(lead) - 0xD800) << 10)
                                        + (u32::from(*low) - 0xDC00)
                                })
                        } else {
                            None
                        };
                        Value::Number(f64::from(paired.unwrap_or_else(|| u32::from(lead))))
                    }
                }
                Native::StringLocaleCompare => {
                    let other = arguments
                        .first()
                        .map_or_else(String::new, Value::to_display_string);
                    Value::Number(match text.encode_utf16().cmp(other.encode_utf16()) {
                        std::cmp::Ordering::Less => -1.0,
                        std::cmp::Ordering::Equal => 0.0,
                        std::cmp::Ordering::Greater => 1.0,
                    })
                }
                // No normalisation tables, so this is the identity. Honest for text that is
                // already normalised, which is what a bundler emits, and wrong for text that is
                // not -- which is why it is called out here rather than left to be discovered.
                Native::StringNormalize => Value::String(text.clone()),
                Native::StringTrimStart => Value::String(text.trim_start().to_owned()),
                Native::StringTrimEnd => Value::String(text.trim_end().to_owned()),
                Native::StringConcat => {
                    let mut joined = text.clone();
                    for argument in arguments {
                        joined.push_str(&argument.to_display_string());
                    }
                    Value::String(joined)
                }
                Native::StringSlice => {
                    // Code units, because `length` reports code units and a program that slices
                    // to `length` has to get the whole string back. Counting characters here
                    // made `s.slice(0, s.length)` shorter than `s` for any string holding an
                    // astral character, and made `slice` disagree with `[i]` about where a
                    // position is -- silently, in both cases.
                    let units: Vec<u16> = text.encode_utf16().collect();
                    let resolve = |value: f64| -> usize {
                        if value < 0.0 {
                            (units.len() as f64 + value).max(0.0) as usize
                        } else {
                            (value as usize).min(units.len())
                        }
                    };
                    let start = resolve(arguments.first().map_or(0.0, Value::as_number));
                    let end = match arguments.get(1) {
                        None | Some(Value::Undefined) => units.len(),
                        Some(value) => resolve(value.as_number()),
                    };
                    Value::String(utf16_slice(&units, start.min(end)..end))
                }
                _ => unreachable!("matched by the arm above"),
            }
        }
        Native::StringValues => {
            let Value::String(text) = receiver else {
                return Ok(Value::Undefined);
            };
            // A string iterator walks code points, not UTF-16 units: an astral character is one
            // step, which is the difference between iterating a string and indexing it.
            let characters = text
                .chars()
                .map(|character| Value::String(character.to_string()))
                .collect::<Vec<_>>();
            let mut iterator = ObjectData::default();
            iterator.set(
                internal_key("iterated"),
                Value::Array(ArrayData::new(characters)),
            );
            iterator.set(internal_key("index"), Value::Number(0.0));
            iterator.set("next", Value::Native(Native::StringIteratorNext));
            iterator.set(
                iterator_key(),
                Value::Native(Native::IteratorSelf),
            );
            Value::Object(Rc::new(RefCell::new(iterator)))
        }
        // The state shape is the array iterator's, so the same stepper serves both.
        Native::StringIteratorNext => call_native(Native::ArrayIteratorNext, receiver, arguments)?,
        Native::SetAdd | Native::MapSet => {
            let Some((entries, is_map)) = collection_state(receiver) else {
                return Ok(Value::Undefined);
            };
            let key = arguments.first().cloned().unwrap_or(Value::Undefined);
            let value = arguments.get(1).cloned().unwrap_or(Value::Undefined);
            let existing = find_entry(&entries.borrow(), &key, is_map);
            match existing {
                Some(index) if is_map => {
                    // Re-setting a key keeps its insertion position, which iteration order shows.
                    if let Some(Value::Array(pair)) = entries.borrow().get(index) {
                        pair.borrow_mut()[1] = value;
                    }
                }
                Some(_) => {}
                None => {
                    let entry = if is_map {
                        Value::Array(ArrayData::new(vec![key, value]))
                    } else {
                        key
                    };
                    entries.borrow_mut().push(entry);
                    bump_size(receiver, 1.0);
                }
            }
            receiver.clone()
        }
        Native::SetHas | Native::MapHas => {
            let Some((entries, is_map)) = collection_state(receiver) else {
                return Ok(Value::Boolean(false));
            };
            let key = arguments.first().cloned().unwrap_or(Value::Undefined);
            let found = find_entry(&entries.borrow(), &key, is_map).is_some();
            Value::Boolean(found)
        }
        Native::MapGet => {
            let Some((entries, is_map)) = collection_state(receiver) else {
                return Ok(Value::Undefined);
            };
            let key = arguments.first().cloned().unwrap_or(Value::Undefined);
            let entries = entries.borrow();
            match find_entry(&entries, &key, is_map) {
                Some(index) => match entries.get(index) {
                    Some(Value::Array(pair)) => {
                        pair.borrow().get(1).cloned().unwrap_or(Value::Undefined)
                    }
                    _ => Value::Undefined,
                },
                None => Value::Undefined,
            }
        }
        Native::CollectionClear => {
            let Some((entries, _)) = collection_state(receiver) else {
                return Ok(Value::Undefined);
            };
            // Every entry becomes a tombstone rather than the vector being emptied, for the same
            // reason a delete does: a live iterator holds an index into this vector, and
            // shortening it underneath one would walk past the end.
            let count = entries.borrow().len();
            for index in 0..count {
                entries.borrow_mut()[index] = Value::Native(Native::Tombstone);
            }
            set_size(receiver, 0.0);
            Value::Undefined
        }
        Native::SetDelete | Native::MapDelete => {
            let Some((entries, is_map)) = collection_state(receiver) else {
                return Ok(Value::Boolean(false));
            };
            let key = arguments.first().cloned().unwrap_or(Value::Undefined);
            let index = find_entry(&entries.borrow(), &key, is_map);
            match index {
                Some(index) => {
                    // A hole, not a removal: compacting here would shift every later element
                    // past a live iterator's cursor and skip one.
                    entries.borrow_mut()[index] = Value::Native(Native::Tombstone);
                    bump_size(receiver, -1.0);
                    Value::Boolean(true)
                }
                None => Value::Boolean(false),
            }
        }
        Native::CollectionValues | Native::CollectionKeys | Native::CollectionEntries => {
            let Some((entries, is_map)) = collection_state(receiver) else {
                return Ok(Value::Undefined);
            };
            let mut iterator = ObjectData::default();
            iterator.set(internal_key("iterated"), Value::Array(entries));
            iterator.set(internal_key("index"), Value::Number(0.0));
            // What to hand back for each stored entry. Decided here, where the collection's kind
            // is known, so the iterator itself needs no reference back to it.
            iterator.set(
                internal_key("part"),
                Value::String(
                    match (is_map, native) {
                        (true, Native::CollectionKeys) => "key",
                        (true, Native::CollectionValues) => "value",
                        (false, Native::CollectionEntries) => "self-pair",
                        _ => "whole",
                    }
                    .to_owned(),
                ),
            );
            iterator.set("next", Value::Native(Native::CollectionIteratorNext));
            iterator.set(
                iterator_key(),
                Value::Native(Native::IteratorSelf),
            );
            Value::Object(Rc::new(RefCell::new(iterator)))
        }
        Native::CollectionIteratorNext => {
            let Value::Object(state) = receiver else {
                return Ok(iteration_result(Value::Undefined, true));
            };
            let (iterated, mut index) = {
                let state = state.borrow();
                (
                    state.get(internal_key("iterated")).unwrap_or(Value::Undefined),
                    state
                        .get(internal_key("index"))
                        .unwrap_or(Value::Number(0.0))
                        .as_number() as usize,
                )
            };
            let Value::Array(entries) = &iterated else {
                return Ok(iteration_result(Value::Undefined, true));
            };
            let part = state
                .borrow()
                .get(internal_key("part"))
                .map_or_else(String::new, |value| value.to_display_string());
            // Skip holes left by deletion, and re-read the length each step so entries added
            // during iteration are seen -- both are observable and both are what a Set does.
            loop {
                let entry = entries.borrow().get(index).cloned();
                match entry {
                    None => {
                        state
                            .borrow_mut()
                            .set(internal_key("index"), Value::Number(index as f64));
                        return Ok(iteration_result(Value::Undefined, true));
                    }
                    Some(Value::Native(Native::Tombstone)) => index += 1,
                    Some(value) => {
                        state
                            .borrow_mut()
                            .set(internal_key("index"), Value::Number(index as f64 + 1.0));
                        let projected = match (part.as_str(), &value) {
                            ("key", Value::Array(pair)) => {
                                pair.borrow().first().cloned().unwrap_or(Value::Undefined)
                            }
                            ("value", Value::Array(pair)) => {
                                pair.borrow().get(1).cloned().unwrap_or(Value::Undefined)
                            }
                            ("self-pair", value) => {
                                Value::Array(ArrayData::new(vec![value.clone(), value.clone()]))
                            }
                            _ => value,
                        };
                        return Ok(iteration_result(projected, false));
                    }
                }
            }
        }
        // Neither is ever called: one marks a deleted slot, the other is thrown by `.return()`.
        Native::Tombstone | Native::ReturnSignal => Value::Undefined,
        Native::ArrayIteratorNext => {
            let Value::Object(state) = receiver else {
                return Ok(iteration_result(Value::Undefined, true));
            };
            let (iterated, index) = {
                let state = state.borrow();
                (
                    state.get(internal_key("iterated")).unwrap_or(Value::Undefined),
                    state
                        .get(internal_key("index"))
                        .unwrap_or(Value::Number(0.0))
                        .as_number(),
                )
            };
            let Value::Array(items) = &iterated else {
                return Ok(iteration_result(Value::Undefined, true));
            };
            let index = index as usize;
            // Re-read the length every step. An array shortened during iteration must stop
            // early rather than read past its end, which is exactly what a cached length would
            // do -- and is the mutation-during-iteration case the corpus checks.
            let item = items.borrow().get(index).cloned();
            match item {
                Some(value) => {
                    state
                        .borrow_mut()
                        .set(internal_key("index"), Value::Number(index as f64 + 1.0));
                    // An iterator walks *indices*, so it visits a hole -- and what it yields
                    // there is what a read yields, which is undefined.
                    iteration_result(
                        if matches!(value, Value::Native(Native::Hole)) {
                            Value::Undefined
                        } else {
                            value
                        },
                        false,
                    )
                }
                None => iteration_result(Value::Undefined, true),
            }
        }
        Native::SetConstructor | Native::MapConstructor => Value::Undefined,
        // `Array(3)` and `new Array(3)` are the same thing, which is unusual enough among the
        // constructors to be worth saying: the specification defines the call form as the
        // construct form, and libraries allocating a sized array reach for the shorter one.
        Native::ArrayConstructor => construct_array(arguments),
        // `Object(x)` boxes a primitive and passes an object through; nothing here has boxes, so
        // an object argument is returned and anything else becomes an empty object.
        Native::ObjectConstructor => match arguments.first() {
            Some(value @ (Value::Object(_) | Value::Array(_) | Value::Function(_))) => {
                value.clone()
            }
            _ => Value::Object(Rc::new(RefCell::new(ObjectData::default()))),
        },
        Native::Error(_)
        | Native::SymbolConstructor
        // A namespace, not a function: `Math` has no call behaviour of its own, and reaching
        // here means something called it.
        | Native::Math
        | Native::Json => Value::Undefined,
    })
}

/// `{ value, done }`, the shape every `next` returns.
fn iteration_result(value: Value, done: bool) -> Value {
    let mut result = ObjectData::default();
    result.set("value", value);
    result.set("done", Value::Boolean(done));
    Value::Object(Rc::new(RefCell::new(result)))
}

/// The trace recorded on an error object when it was constructed, if it is one.
fn captured_trace(thrown: &Value) -> Vec<String> {
    let Value::Object(object) = thrown else {
        return Vec::new();
    };
    match object.borrow().get_own(internal_key("trace")) {
        Some(Value::Array(frames)) => frames
            .borrow()
            .iter()
            .map(Value::to_display_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// How an uncaught value is described. An error renders through its `toString`; anything else
/// through its ordinary string form, because JavaScript lets you throw any value.
fn describe_thrown(value: &Value) -> String {
    if has_error_to_string(value) {
        error_to_string(value)
    } else {
        value.to_display_string()
    }
}

/// Whether a value inherits `Error.prototype.toString`.
fn has_error_to_string(value: &Value) -> bool {
    match value {
        Value::Object(object) => matches!(
            object.borrow().get("toString"),
            Some(Value::Native(Native::ErrorToString))
        ),
        _ => false,
    }
}

/// `Error.prototype.toString`: `name`, then `: message` when the message is non-empty.
fn error_to_string(value: &Value) -> String {
    let Value::Object(object) = value else {
        return "Error".to_owned();
    };
    let object = object.borrow();
    let name = match object.get("name") {
        Some(Value::Undefined) | None => "Error".to_owned(),
        Some(other) => other.to_display_string(),
    };
    let message = match object.get("message") {
        Some(Value::Undefined) | None => String::new(),
        Some(other) => other.to_display_string(),
    };
    if message.is_empty() {
        name
    } else {
        format!("{name}: {message}")
    }
}

/// `value instanceof constructor`.
///
/// Walks the value's prototype chain looking for the constructor's `prototype` *by identity*.
/// Comparing anything else -- shape, name -- would call two unrelated classes the same one.
///
/// A right-hand side that is not a function is a TypeError in JavaScript. There is nothing to
/// throw with yet, so it answers false; that is wrong, and it is the direction that produces a
/// visibly wrong answer rather than a silently corrupt one.
fn instance_of(value: &Value, constructor: &Value, error_prototype: Option<&Value>) -> bool {
    let prototype = match constructor {
        Value::Function(closure) => match closure.prototype.borrow().clone() {
            Some(prototype) => prototype,
            // A constructor never used with `new` and never read has no prototype object yet,
            // so nothing can descend from it.
            None => return false,
        },
        // A native constructor's prototype is the realm's, not one per closure.
        Value::Native(Native::Error(_)) => match error_prototype {
            Some(prototype) => prototype.clone(),
            None => return false,
        },
        // These have no prototype object to walk -- each is its own value kind -- so the test is
        // what the chain would have answered.
        Value::Native(Native::PromiseConstructor) => {
            return matches!(value, Value::Promise(_));
        }
        Value::Native(Native::DateConstructor) => return matches!(value, Value::Date(_)),
        Value::Native(Native::RegExpConstructor) => return matches!(value, Value::RegExp(_)),
        Value::Native(Native::ArrayConstructor) => return matches!(value, Value::Array(_)),
        // A view is an instance of the constructor that names its element type, and of nothing
        // else: `new Uint8Array(1) instanceof Int8Array` is false.
        Value::Native(Native::TypedArrayConstructor(kind)) => {
            return matches!(value, Value::TypedArray(view) if view.kind == *kind);
        }
        Value::Native(Native::ArrayBufferConstructor) => {
            return matches!(value, Value::ArrayBuffer(_));
        }
        // A Set or Map is an object carrying reserved state, so the test is for that state
        // rather than for a prototype the realm does not yet have as an object.
        Value::Native(kind @ (Native::SetConstructor | Native::MapConstructor)) => {
            let Value::Object(object) = value else {
                return false;
            };
            let object = object.borrow();
            return object.get_own(internal_key("entries")).is_some()
                && object.get_own(internal_key("map")).is_some()
                    == matches!(kind, Native::MapConstructor);
        }
        // Any other built-in that has a `prototype`: the chain is walked against it, which is
        // what makes `Object instanceof Object` true -- `Object` is a function, so its chain
        // reaches `Function.prototype` and then `Object.prototype`, which is what it is being
        // compared against. A plain-object check asks exactly that.
        Value::Native(native) => match native.property("prototype") {
            Some(table) => table,
            None => return false,
        },
        _ => return false,
    };

    // Bounded like every other prototype walk: the chain belongs to the program, and running out
    // of Rust stack -- or looping -- is not a way to answer a question.
    let mut current = prototype_of(value);
    for _ in 0..MAX_PROTOTYPE_DEPTH {
        if matches!(current, Value::Null) {
            return false;
        }
        if strict_equal(&current, &prototype) {
            return true;
        }
        current = prototype_of(&current);
    }
    false
}

/// A value's `[[Prototype]]`, or `Value::Null` at the end of the chain.
///
/// One function so `Object.getPrototypeOf` and `instanceof` cannot disagree about the shape of
/// the chain -- which is the sort of disagreement that makes `x instanceof C` false for an `x`
/// the program just built.
fn prototype_of(value: &Value) -> Value {
    let recorded = match value {
        Value::Object(object) => object.borrow().prototype().cloned(),
        // A subclassed array keeps its prototype with its other non-element properties, which is
        // what makes `new (class extends Array {})() instanceof that class` true.
        Value::Array(array) => array.properties.borrow().prototype().cloned(),
        Value::Function(closure) => closure.properties.borrow().prototype().cloned(),
        _ => None,
    };
    if let Some(link) = recorded {
        return link;
    }
    match value {
        // `Object.prototype` is the end of every chain; the other tables inherit from it.
        Value::Native(Native::PrototypeOf(PrototypeTable::Object)) => Value::Null,
        Value::Native(Native::PrototypeOf(_)) => {
            Value::Native(Native::PrototypeOf(PrototypeTable::Object))
        }
        // Every callable inherits `Function.prototype`, and every ordinary object
        // `Object.prototype`. A primitive has no chain of its own to walk.
        Value::Native(_) | Value::Bound(_) | Value::Callback(_) | Value::Function(_) => {
            Value::Native(Native::PrototypeOf(PrototypeTable::Function))
        }
        Value::Array(_) => Value::Native(Native::PrototypeOf(PrototypeTable::Array)),
        Value::Object(_) => Value::Native(Native::PrototypeOf(PrototypeTable::Object)),
        _ => Value::Null,
    }
}

/// `key in target`.
fn has_property(target: &Value, key: &Value) -> bool {
    // `1 in [1, , 3]` is false and `1 in [1, undefined, 3]` is true: the one place the difference
    // between a hole and an undefined element is directly observable.
    if let Value::Array(items) = target
        && let Some(index) = array_index(key)
    {
        return items
            .borrow()
            .get(index)
            .is_some_and(|value| !matches!(value, Value::Native(Native::Hole)));
    }
    let typed = key.to_property_key();
    let Some(name) = string_key(&typed) else {
        return match target {
            Value::Object(object) => object.borrow().lookup(typed).is_some(),
            Value::Function(closure) => closure.properties.borrow().lookup(typed).is_some(),
            Value::Array(items) => {
                items.properties.borrow().lookup(typed.clone()).is_some() || typed == iterator_key()
            }
            Value::String(_) => typed == iterator_key(),
            Value::Generator(state) => typed == iterator_key() && !is_async_generator(state),
            _ => false,
        };
    };
    // Every intrinsic kind answers `constructor` with the built-in that makes it, so `in` has to
    // agree with the read rather than only with the property store.
    if name == "constructor"
        && (intrinsic_constructor(target).is_some() || matches!(target, Value::Object(_)))
    {
        return true;
    }
    match target {
        // The chain ends at `Object.prototype`, whose methods every object has -- so
        // `"toString" in x` is true, and so is the `"hasOwnProperty" in x` that half the
        // feature-detects in the wild are written as. Answering from the own store alone made
        // both false, which is a wrong answer that reads like a deliberate one.
        Value::Object(object) => {
            object.borrow().lookup(typed.clone()).is_some()
                || !matches!(inherited_object_property(name), Value::Undefined)
        }
        Value::Function(closure) => {
            closure.properties.borrow().lookup(typed.clone()).is_some()
                || !matches!(function_property(name), Value::Undefined)
        }
        Value::Array(items) => {
            if let Some(index) = array_index(key) {
                return index < items.borrow().len();
            }
            if items.properties.borrow().lookup(typed.clone()).is_some() {
                return true;
            }
            // Methods and `length` live on the prototype in a real engine; here the property
            // lookup answers for both, so `in` has to agree with it.
            name == "length"
                || !matches!(array_property(name), Value::Undefined)
                || !matches!(inherited_object_property(name), Value::Undefined)
        }
        Value::Native(native) => native.property(name).is_some(),
        _ => false,
    }
}

fn own_enumerable(target: &Value, key: &Value) -> bool {
    let key = key.to_property_key();
    match target {
        Value::Object(object) => object
            .borrow()
            .enumerable_entries()
            .any(|(name, _)| name == &key),
        Value::Function(closure) => closure
            .properties
            .borrow()
            .enumerable_entries()
            .any(|(name, _)| name == &key),
        Value::Array(items) => {
            if let Some(index) = array_index(&key_value(&key)) {
                return items.borrow().get(index).is_some_and(|v| {
                    !matches!(v, Value::Native(Native::Hole)) && items.index_enumerable(index)
                });
            }
            items
                .properties
                .borrow()
                .enumerable_entries()
                .any(|(name, _)| name == &key)
        }
        _ => false,
    }
}

fn own_property_exists(target: &Value, key: &Value) -> bool {
    let key = key.to_property_key();
    match target {
        Value::Object(object) => object.borrow().get_own_property(key).is_some(),
        Value::Function(closure) => closure.properties.borrow().get_own_property(key).is_some(),
        Value::Array(items) => {
            if let Some(index) = string_index(&key) {
                return items
                    .borrow()
                    .get(index as usize)
                    .is_some_and(|value| !matches!(value, Value::Native(Native::Hole)));
            }
            if key == PropertyKey::String("length".to_owned()) {
                return true;
            }
            items.properties.borrow().get_own_property(key).is_some()
        }
        Value::String(text) => match key {
            PropertyKey::String(name) if name == "length" => true,
            PropertyKey::String(name) => string_index(&PropertyKey::String(name.clone()))
                .is_some_and(|index| (index as usize) < text.encode_utf16().count()),
            _ => false,
        },
        _ => false,
    }
}

fn set_target_enumerable(target: &Value, key: &PropertyKey, enumerable: bool) {
    match target {
        Value::Object(object) => object.borrow_mut().set_enumerable(key, enumerable),
        Value::Function(closure) => closure
            .properties
            .borrow_mut()
            .set_enumerable(key, enumerable),
        Value::Array(items) => {
            if let Some(index) = array_index(&key_value(key)) {
                items.set_index_enumerable(index, enumerable);
            } else {
                items
                    .properties
                    .borrow_mut()
                    .set_enumerable(key, enumerable);
            }
        }
        _ => {}
    }
}

/// The properties an array exposes, shared by reads and by `in` so the two cannot disagree.
/// The index of `key` among live entries, or `None`.
fn find_entry(entries: &[Value], key: &Value, is_map: bool) -> Option<usize> {
    entries.iter().position(|entry| match entry {
        Value::Native(Native::Tombstone) => false,
        Value::Array(pair) if is_map => pair
            .borrow()
            .first()
            .is_some_and(|existing| same_value_zero(existing, key)),
        other => !is_map && same_value_zero(other, key),
    })
}

/// Adjust a collection's stored `size`.
///
/// Stored rather than derived so a read is a plain property lookup; the two cannot drift because
/// only `add` and `delete` change either.
fn set_size(receiver: &Value, size: f64) {
    if let Value::Object(object) = receiver {
        object.borrow_mut().set("size", Value::Number(size));
    }
}

fn bump_size(receiver: &Value, delta: f64) {
    if let Value::Object(object) = receiver {
        let current = object
            .borrow()
            .get_own("size")
            .map_or(0.0, Value::as_number);
        object
            .borrow_mut()
            .set("size", Value::Number(current + delta));
    }
}

/// The methods a string exposes.
///
/// Strings are primitives here rather than objects with a prototype, so their methods are looked
/// up directly. When `String.prototype` becomes a real object -- which subclassing and
/// monkey-patching both need -- this table is what moves onto it.
fn string_property(name: &str) -> Value {
    match name {
        "values" => Value::Native(Native::StringValues),
        "toUpperCase" => Value::Native(Native::StringToUpperCase),
        "toLowerCase" => Value::Native(Native::StringToLowerCase),
        "includes" => Value::Native(Native::StringIncludes),
        "indexOf" => Value::Native(Native::StringIndexOf),
        "slice" => Value::Native(Native::StringSlice),
        "substring" => Value::Native(Native::StringSubstring),
        // Deprecated in the specification and still everywhere in the wild.
        "substr" => Value::Native(Native::StringSubstr),
        "split" => Value::Native(Native::StringSplit),
        "trim" => Value::Native(Native::StringTrim),
        "startsWith" => Value::Native(Native::StringStartsWith),
        "endsWith" => Value::Native(Native::StringEndsWith),
        "repeat" => Value::Native(Native::StringRepeat),
        "at" => Value::Native(Native::StringAt),
        "codePointAt" => Value::Native(Native::StringCodePointAt),
        // No locale data, so this is the ordinary code-unit order -- which is what a program
        // sorting ASCII gets from either.
        "localeCompare" => Value::Native(Native::StringLocaleCompare),
        "toLocaleUpperCase" => Value::Native(Native::StringToUpperCase),
        "toLocaleLowerCase" => Value::Native(Native::StringToLowerCase),
        "normalize" => Value::Native(Native::StringNormalize),
        "charAt" => Value::Native(Native::StringCharAt),
        "charCodeAt" => Value::Native(Native::StringCharCodeAt),
        "padStart" => Value::Native(Native::StringPadStart),
        "padEnd" => Value::Native(Native::StringPadEnd),
        "replace" => Value::Native(Native::StringReplace),
        "replaceAll" => Value::Native(Native::StringReplaceAll),
        "lastIndexOf" => Value::Native(Native::StringLastIndexOf),
        "trimStart" => Value::Native(Native::StringTrimStart),
        "trimEnd" => Value::Native(Native::StringTrimEnd),
        "concat" => Value::Native(Native::StringConcat),
        // A string's `toString` and `valueOf` both hand back the string itself. They are
        // rarely written by hand and constantly reached by library code that calls `toString`
        // on something it has already coerced.
        "toString" | "toLocaleString" | "valueOf" => Value::Native(Native::ObjectValueOf),
        "match" => Value::Native(Native::StringMatch),
        "matchAll" => Value::Native(Native::StringMatchAll),
        "search" => Value::Native(Native::StringSearch),
        _ => Value::Undefined,
    }
}

/// The methods a Set or Map instance exposes, shared by property reads and `in`.
/// The methods a Set or Map instance exposes, shared by property reads and `in`.
///
/// A weak collection exposes only the four that take a key: it has no size and cannot be walked,
/// because with no way to observe collection there would be nothing to stop a program depending
/// on entries a real engine would already have dropped.
fn collection_property(is_map: bool, weak: bool, name: &str) -> Value {
    if weak && matches!(name, "values" | "keys" | "entries" | "forEach") {
        return Value::Undefined;
    }
    match (is_map, name) {
        (false, "add") => Value::Native(Native::SetAdd),
        (false, "has") => Value::Native(Native::SetHas),
        (false, "delete") => Value::Native(Native::SetDelete),
        (true, "set") => Value::Native(Native::MapSet),
        (true, "get") => Value::Native(Native::MapGet),
        (true, "has") => Value::Native(Native::MapHas),
        (true, "delete") => Value::Native(Native::MapDelete),
        // A Map stores `[key, value]` pairs, so `keys` and `values` project out of them where
        // `entries` hands the pair over whole. A Set stores bare values, and its `entries` pairs
        // each one with itself -- which is the specification's way of giving the two collections
        // one iteration protocol.
        (_, "clear") => Value::Native(Native::CollectionClear),
        (_, "forEach") => Value::Native(Native::CollectionForEach),
        (_, "values") => Value::Native(Native::CollectionValues),
        (_, "keys") => Value::Native(Native::CollectionKeys),
        (_, "entries") => Value::Native(Native::CollectionEntries),
        _ => Value::Undefined,
    }
}

/// The backing entries of a Set or Map, and whether it is a Map.
fn collection_state(receiver: &Value) -> Option<(Rc<ArrayData>, bool)> {
    let Value::Object(object) = receiver else {
        return None;
    };
    let object = object.borrow();
    let Some(Value::Array(entries)) = object.get_own(internal_key("entries")).cloned() else {
        return None;
    };
    let is_map = matches!(
        object.get_own(internal_key("map")),
        Some(&Value::Boolean(true))
    );
    Some((entries, is_map))
}

/// SameValueZero, which is what Set and Map use for key identity.
///
/// It differs from `===` in exactly one place: NaN is its own key, so a Set can contain one and
/// find it again. `-0` and `0` remain the same key under both.
fn same_value_zero(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) if a.is_nan() && b.is_nan() => true,
        _ => strict_equal(left, right),
    }
}

/// Array methods that call back into user code, and so cannot be plain natives.
fn array_callback_method(name: &str) -> Option<CallbackMethod> {
    match name {
        "map" => Some(CallbackMethod::Map),
        "forEach" => Some(CallbackMethod::ForEach),
        "filter" => Some(CallbackMethod::Filter),
        "reduce" => Some(CallbackMethod::Reduce),
        "sort" => Some(CallbackMethod::Sort),
        "findIndex" => Some(CallbackMethod::FindIndex),
        "find" => Some(CallbackMethod::Find),
        "some" => Some(CallbackMethod::Some),
        "every" => Some(CallbackMethod::Every),
        "flatMap" => Some(CallbackMethod::FlatMap),
        "findLast" => Some(CallbackMethod::FindLast),
        "findLastIndex" => Some(CallbackMethod::FindLastIndex),
        "reduceRight" => Some(CallbackMethod::ReduceRight),
        _ => None,
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
    /// `next` on an async generator: hands back a promise of the step.
    AsyncGeneratorNext,
    Map,
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

/// The methods a number primitive exposes.
///
/// Read off the value rather than through `Number.prototype`, which does not exist yet. Unbound,
/// like the string and array tables, so `CallMethod` supplies the receiver.
fn number_property(name: &str) -> Value {
    match name {
        "toString" => Value::Native(Native::NumberToString),
        "toFixed" => Value::Native(Native::NumberToFixed),
        "valueOf" => Value::Native(Native::NumberValueOf),
        _ => Value::Undefined,
    }
}

fn array_property(name: &str) -> Value {
    if let Some(method) = array_callback_method(name) {
        return Value::Callback(method);
    }
    match name {
        "join" => Value::Native(Native::ArrayJoin),
        "push" => Value::Native(Native::ArrayPush),
        "pop" => Value::Native(Native::ArrayPop),
        "indexOf" => Value::Native(Native::ArrayIndexOf),
        "includes" => Value::Native(Native::ArrayIncludes),
        "slice" => Value::Native(Native::ArraySlice),
        "at" => Value::Native(Native::ArrayAt),
        "flat" => Value::Native(Native::ArrayFlat),
        "entries" => Value::Native(Native::ArrayEntries),
        "keys" => Value::Native(Native::ArrayKeys),
        "concat" => Value::Native(Native::ArrayConcat),
        "reverse" => Value::Native(Native::ArrayReverse),
        "shift" => Value::Native(Native::ArrayShift),
        "unshift" => Value::Native(Native::ArrayUnshift),
        "lastIndexOf" => Value::Native(Native::ArrayLastIndexOf),
        "fill" => Value::Native(Native::ArrayFill),
        "splice" => Value::Native(Native::ArraySplice),
        "copyWithin" => Value::Native(Native::ArrayCopyWithin),
        // `Array.prototype.toString` is `join(",")`, which is why `String([1, 2])` is `"1,2"`.
        "toString" => Value::Native(Native::ArrayJoinDefault),
        "values" => Value::Native(Native::ArrayValues),
        _ => Value::Undefined,
    }
}

/// A key usable as a dense array index: a non-negative integer that fits in `usize`.
///
/// `a["0"]` and `a[0]` name the same element, so a string that round-trips through a number is
/// an index too. `a[-1]` and `a[1.5]` are ordinary string properties in JavaScript, and treating
/// them as indices would silently move data.
fn array_index(key: &Value) -> Option<usize> {
    let number = match key {
        Value::Number(value) => *value,
        Value::String(text) => text.parse::<f64>().ok()?,
        _ => return None,
    };
    if number < 0.0 || number.fract() != 0.0 || !number.is_finite() {
        return None;
    }
    Some(number as usize)
}

/// `Array.prototype.join` semantics: null and undefined become empty, everything else stringifies.
/// A hole and `undefined` both render as nothing here, which is what `join` and `String()` do.
fn join(items: &[Value], separator: &str) -> String {
    items
        .iter()
        .map(|item| match item {
            Value::Undefined | Value::Null => String::new(),
            Value::Native(Native::Hole) => String::new(),
            other => other.to_display_string(),
        })
        .collect::<Vec<_>>()
        .join(separator)
}

/// Remove an own property, reporting whether it is gone afterwards.
///
/// Which is why deleting something that was never there is still true, and why deleting a
/// string's `length` is false: the property is real and cannot be configured away. Nothing else
/// in this VM has a non-configurable property yet, so a string is the only case that says no.
fn delete_property(target: &Value, key: &Value) -> bool {
    match target {
        Value::Object(object) => {
            object.borrow_mut().remove(key.to_property_key());
            true
        }
        Value::Function(closure) => {
            closure
                .properties
                .borrow_mut()
                .remove(key.to_property_key());
            true
        }
        // Removing an element leaves a *hole* rather than shortening the array: the slot reads
        // as undefined and `length` is unchanged, but `1 in a` is false and a console shows
        // `<1 empty item>`. The comment said hole and the code wrote undefined, which is the
        // one place the difference between the two is directly observable.
        Value::Array(items) => {
            if let Some(index) = array_index(key) {
                let exists = items
                    .borrow()
                    .get(index)
                    .is_some_and(|value| !matches!(value, Value::Native(Native::Hole)));
                if !exists {
                    items.remove_index_attributes(index);
                    return true;
                }
                if let Some(slot) = items.borrow_mut().get_mut(index) {
                    *slot = Value::Native(Native::Hole);
                    items.remove_index_attributes(index);
                }
            } else {
                items.properties.borrow_mut().remove(key.to_property_key());
            }
            true
        }
        Value::String(text) => {
            let key = key.property_key();
            let indexed = string_index(&PropertyKey::String(key.clone()))
                .is_some_and(|index| (index as usize) < text.encode_utf16().count());
            !(key == "length" || indexed)
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
fn own_properties(value: &Value) -> Vec<(PropertyKey, Property)> {
    match value {
        Value::Object(object) => object
            .borrow()
            .enumerable_entries()
            .filter(|(key, _)| !matches!(key, PropertyKey::Internal(_)))
            .map(|(key, property)| (key.clone(), property.clone()))
            .collect(),
        Value::Function(closure) => closure
            .properties
            .borrow()
            .enumerable_entries()
            .filter(|(key, _)| !matches!(key, PropertyKey::Internal(_)))
            .map(|(key, property)| (key.clone(), property.clone()))
            .collect(),
        Value::Array(items) => {
            let mut properties = items
                .borrow()
                .iter()
                .enumerate()
                .filter(|(index, item)| {
                    !matches!(item, Value::Native(Native::Hole)) && items.index_enumerable(*index)
                })
                .map(|(index, item)| {
                    (
                        PropertyKey::String(index.to_string()),
                        Property::Value(item.clone()),
                    )
                })
                .collect::<Vec<_>>();
            properties.extend(
                items
                    .properties
                    .borrow()
                    .enumerable_entries()
                    .filter(|(key, _)| !matches!(key, PropertyKey::Internal(_)))
                    .map(|(key, property)| (key.clone(), property.clone())),
            );
            properties
        }
        Value::String(text) => text
            .chars()
            .enumerate()
            .map(|(index, character)| {
                (
                    PropertyKey::String(index.to_string()),
                    Property::Value(Value::String(character.to_string())),
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A number written in a base other than ten.
///
/// The fraction is expanded by repeated multiplication rather than converted through a decimal
/// string, because there is no decimal spelling of `0.1` in base 3 to convert. It stops at 52
/// digits, which is where the mantissa runs out and every further digit would be invented.
fn number_in_radix(value: f64, radix: u32) -> String {
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

/// The first argument as a number, or NaN when there is none.
///
/// `Math.abs()` is NaN rather than 0: a missing argument is `undefined`, and `Number(undefined)`
/// is NaN.
fn argument_number(arguments: &[Value]) -> f64 {
    arguments.first().map_or(f64::NAN, Value::as_number)
}

fn is_integer(value: f64) -> bool {
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
fn number_to_fixed(value: f64, digits: u32) -> String {
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

/// `parseInt(text, radix?)`.
///
/// Reads as far as it can and stops, rather than rejecting: `parseInt("12px")` is 12. That
/// tolerance is the entire difference from `Number("12px")`, which is NaN.
fn parse_int(arguments: &[Value]) -> Value {
    let text = arguments
        .first()
        .map_or_else(String::new, Value::to_display_string);
    let text = text.trim();
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
fn parse_float(arguments: &[Value]) -> Value {
    let text = arguments
        .first()
        .map_or_else(String::new, Value::to_display_string);
    let text = text.trim_start();
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
fn relative_index(argument: Option<&Value>, length: usize, default: usize) -> usize {
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
/// Hand-written rather than delegating to serde: the output is a `Value` graph with JavaScript's
/// own rules -- every number is an `f64`, objects keep insertion order, and duplicate keys take
/// the last value -- and going through an intermediate representation would mean translating all
/// three back afterwards.
struct JsonReader<'text> {
    rest: &'text str,
}

impl JsonReader<'_> {
    fn skip_whitespace(&mut self) {
        // JSON's whitespace is exactly these four; anything else is a token.
        self.rest = self.rest.trim_start_matches([' ', '\t', '\n', '\r']);
    }

    /// Consume `token` if it is next.
    fn eat(&mut self, token: char) -> bool {
        match self.rest.strip_prefix(token) {
            Some(rest) => {
                self.rest = rest;
                true
            }
            None => false,
        }
    }

    fn value(&mut self) -> Result<Value, ExecutionError> {
        self.skip_whitespace();
        let Some(next) = self.rest.chars().next() else {
            return Err(ExecutionError::InvalidJson);
        };
        match next {
            '{' => self.object(),
            '[' => self.array(),
            '"' => self.string().map(Value::String),
            't' | 'f' | 'n' => self.keyword(),
            _ => self.number(),
        }
    }

    fn keyword(&mut self) -> Result<Value, ExecutionError> {
        for (word, value) in [
            ("true", Value::Boolean(true)),
            ("false", Value::Boolean(false)),
            ("null", Value::Null),
        ] {
            if let Some(rest) = self.rest.strip_prefix(word) {
                self.rest = rest;
                return Ok(value);
            }
        }
        Err(ExecutionError::InvalidJson)
    }

    fn number(&mut self) -> Result<Value, ExecutionError> {
        let end = self
            .rest
            .find(|character: char| !matches!(character, '0'..='9' | '-' | '+' | '.' | 'e' | 'E'))
            .unwrap_or(self.rest.len());
        let (text, rest) = self.rest.split_at(end);
        // `f64::from_str` accepts more than JSON does -- "inf", "NaN", a leading "+" -- so the
        // characters were bounded above first and the spellings JSON forbids are rejected here.
        let number: f64 = text.parse().map_err(|_| ExecutionError::InvalidJson)?;
        if !number.is_finite() || text.starts_with('+') || text.starts_with(".") {
            return Err(ExecutionError::InvalidJson);
        }
        self.rest = rest;
        Ok(Value::Number(number))
    }

    fn string(&mut self) -> Result<String, ExecutionError> {
        if !self.eat('"') {
            return Err(ExecutionError::InvalidJson);
        }
        let mut text = String::new();
        loop {
            let mut characters = self.rest.chars();
            let Some(character) = characters.next() else {
                return Err(ExecutionError::InvalidJson);
            };
            self.rest = characters.as_str();
            match character {
                '"' => return Ok(text),
                '\\' => text.push(self.escape()?),
                // A raw control character is not allowed in a JSON string; it has to be escaped.
                control if control < ' ' => return Err(ExecutionError::InvalidJson),
                other => text.push(other),
            }
        }
    }

    /// The four hex digits after a `\u`, consumed.
    fn code_unit(&mut self) -> Result<u32, ExecutionError> {
        let (digits, rest) = self
            .rest
            .split_at_checked(4)
            .ok_or(ExecutionError::InvalidJson)?;
        let code = u32::from_str_radix(digits, 16).map_err(|_| ExecutionError::InvalidJson)?;
        self.rest = rest;
        Ok(code)
    }

    fn escape(&mut self) -> Result<char, ExecutionError> {
        let mut characters = self.rest.chars();
        let Some(marker) = characters.next() else {
            return Err(ExecutionError::InvalidJson);
        };
        self.rest = characters.as_str();
        Ok(match marker {
            '"' => '"',
            '\\' => '\\',
            '/' => '/',
            'b' => '\u{8}',
            'f' => '\u{c}',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'u' => {
                let code = self.code_unit()?;
                // A pair is two escapes and one character. Decoding each half on its own gave
                // two replacement characters for every emoji that arrived escaped, which is how
                // a JSON document written by any other engine spells one outside the BMP.
                if (0xD800..0xDC00).contains(&code)
                    && let Some(rest) = self.rest.strip_prefix("\\u")
                {
                    let saved = self.rest;
                    self.rest = rest;
                    let low = self.code_unit()?;
                    if (0xDC00..0xE000).contains(&low) {
                        let combined = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                        return Ok(char::from_u32(combined).unwrap_or(char::REPLACEMENT_CHARACTER));
                    }
                    // Not a low half after all, so the second escape is not ours to consume.
                    self.rest = saved;
                }
                // A lone surrogate has no scalar value. Replacing it keeps the string readable
                // rather than failing the whole parse over one unpaired half.
                char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER)
            }
            _ => return Err(ExecutionError::InvalidJson),
        })
    }

    fn array(&mut self) -> Result<Value, ExecutionError> {
        self.eat('[');
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.eat(']') {
            return Ok(Value::Array(ArrayData::new(items)));
        }
        loop {
            items.push(self.value()?);
            self.skip_whitespace();
            if self.eat(']') {
                return Ok(Value::Array(ArrayData::new(items)));
            }
            if !self.eat(',') {
                return Err(ExecutionError::InvalidJson);
            }
        }
    }

    fn object(&mut self) -> Result<Value, ExecutionError> {
        self.eat('{');
        let mut data = ObjectData::default();
        self.skip_whitespace();
        if self.eat('}') {
            return Ok(Value::Object(Rc::new(RefCell::new(data))));
        }
        loop {
            self.skip_whitespace();
            let key = self.string()?;
            self.skip_whitespace();
            if !self.eat(':') {
                return Err(ExecutionError::InvalidJson);
            }
            let value = self.value()?;
            data.set(&key, value);
            self.skip_whitespace();
            if self.eat('}') {
                return Ok(Value::Object(Rc::new(RefCell::new(data))));
            }
            if !self.eat(',') {
                return Err(ExecutionError::InvalidJson);
            }
        }
    }
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
fn inspect(value: &Value) -> String {
    match value {
        // A top-level string prints as itself; only a nested one is quoted.
        Value::String(text) => text.clone(),
        other => inspect_nested(other, 0, &mut Vec::new()),
    }
}

/// How deep node descends before printing a placeholder instead of the contents.
const INSPECT_DEPTH: usize = 2;

fn inspect_nested(value: &Value, depth: usize, seen: &mut Vec<usize>) -> String {
    match value {
        Value::String(text) => quote_for_console(text),
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
        Value::Native(native) => match namespace_name(native) {
            Some(name) => format!("Object [{name}] {{}}"),
            None => format!("[Function: {}]", native_js_name(native)),
        },
        Value::Callback(method) => {
            format!("[Function: {}]", builtin_js_name(&format!("{method:?}")))
        }
        Value::Generator(_) => "Object [Generator] {}".to_owned(),
        Value::Settler(..) => "[Function (anonymous)]".to_owned(),
        // Node prints the target's name with a "bound " prefix; without a name to prefix, the
        // fact that it is bound is the whole of what there is to say.
        Value::Bound(_) => "[Function: bound]".to_owned(),
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
        Value::ArrayBuffer(bytes) => {
            format!("ArrayBuffer {{ byteLength: {} }}", bytes.borrow().len())
        }
        Value::RegExp(regexp) => format!("/{}/{}", regexp.source, regexp.flags),
        Value::Date(time) => iso_string(time.get()).unwrap_or_else(|| "Invalid Date".to_owned()),
        // A console shows the literal's spelling, which is how it is told from a number.
        Value::BigInt(value) => format!("{value}n"),
        Value::Promise(promise) => match &promise.borrow().settled {
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
                .enumerable_entries()
                .filter(|(key, _)| !matches!(key, PropertyKey::Internal(_)))
                .map(|(key, property)| {
                    let shown = match property {
                        Property::Value(value) => inspect_nested(value, depth + 1, guard.seen),
                        Property::Accessor { .. } => "[Getter/Setter]".to_owned(),
                    };
                    format!("{}: {}", inspect_key(key), shown)
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
                if matches!(item, Value::Native(Native::Hole)) {
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
        Value::Cell(_) => unreachable!("a cell is never inspected"),
    }
}

fn inspect_object(object: &ObjectData, depth: usize, seen: &mut Vec<usize>) -> String {
    // A Set or Map is an object carrying reserved state, and a console shows what it holds
    // rather than the bookkeeping that makes it work.
    if let Some(Value::Array(entries)) = object.get_own(internal_key("entries")) {
        let is_map = matches!(
            object.get_own(internal_key("map")),
            Some(&Value::Boolean(true))
        );
        let live: Vec<_> = entries
            .borrow()
            .iter()
            .filter(|entry| !matches!(entry, Value::Native(Native::Tombstone)))
            .cloned()
            .collect();
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
        .enumerable_entries()
        .filter(|(key, _)| !matches!(key, PropertyKey::Internal(_)))
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
            format!("{}: {}", inspect_key(key), shown)
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
fn constructor_label(object: &ObjectData) -> Option<String> {
    let constructor = object.get("constructor")?;
    let name = match &constructor {
        Value::Function(closure) => closure.name.as_deref()?.to_owned(),
        Value::Native(native) => format!("{native:?}"),
        _ => return None,
    };
    (!name.is_empty() && name != "Object").then_some(name)
}

/// Compile a regular expression, or `None` when the engine rejects the pattern.
fn build_regexp(pattern: &str, flags: &str) -> Option<Value> {
    let compiled = regress::Regex::with_flags(pattern, flags).ok()?;
    Some(Value::RegExp(Rc::new(RegExpData {
        source: pattern.to_owned(),
        flags: flags.to_owned(),
        compiled,
        global: flags.contains('g'),
        sticky: flags.contains('y'),
        last_index: std::cell::Cell::new(0),
    })))
}

/// Run a regular expression against a subject from a given UTF-16 offset.
///
/// Returns the match's range and its capture groups, all in UTF-16 units -- which is what the
/// language counts in, and why the subject is converted rather than matched as UTF-8.
fn regexp_search(regexp: &RegExpData, subject: &[u16], start: usize) -> Option<RegExpMatch> {
    if start > subject.len() {
        return None;
    }
    let found = regexp.compiled.find_from_utf16(subject, start).next()?;
    // A sticky pattern must match exactly at the cursor, not merely at or after it.
    if regexp.sticky && found.range().start != start {
        return None;
    }
    let groups = found.groups().collect();
    let named = found
        .named_groups()
        .map(|(name, range)| (name.to_owned(), range))
        .collect();
    Some((found.range(), groups, named))
}

/// `new Array(...)`: a length when given one number, the elements themselves otherwise.
///
/// The one-argument form is the language's oldest ambiguity -- `new Array(3)` is three holes and
/// `new Array('3')` is one string -- and it is not this compiler's to fix.
fn construct_array(arguments: &[Value]) -> Value {
    if let [Value::Number(length)] = arguments {
        let length = *length;
        if length < 0.0 || length.fract() != 0.0 || length > 4_294_967_295.0 {
            // A non-integer or out-of-range length is a RangeError in the specification; an
            // empty array is closer to that than a silently truncated one.
            return Value::Array(ArrayData::new(Vec::new()));
        }
        return Value::Array(ArrayData::new(vec![
            Value::Native(Native::Hole);
            length as usize
        ]));
    }
    Value::Array(ArrayData::new(arguments.to_vec()))
}

/// The civil date a count of days since the epoch names.
///
/// Hinnant's algorithm, which is exact for the whole range a `f64` instant can reach and needs no
/// tables. Shifting the year to start in March is what makes the leap day the last day of the
/// year and removes every special case from the month arithmetic.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    (year + i64::from(month <= 2), month, day)
}

/// The inverse of [`civil_from_days`], for `Date.UTC` and the constructor's field form.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Which part of an instant a reader returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateField {
    Time,
    FullYear,
    Month,
    Day,
    Weekday,
    Hours,
    Minutes,
    Seconds,
    Milliseconds,
    /// Always zero: the realm has no zone but UTC, so there is nothing to offset by.
    TimezoneOffset,
}

/// The parts of an instant, all in UTC -- see ADR 0015 for why the realm has no other zone.
struct DateParts {
    year: i64,
    month: u32,
    day: u32,
    weekday: u32,
    hour: u32,
    minute: u32,
    second: u32,
    millisecond: u32,
}

fn date_parts(time: f64) -> Option<DateParts> {
    if !time.is_finite() {
        return None;
    }
    let millis = time as i64;
    // Floor division, not truncation: an instant before the epoch is a negative count and must
    // round *down* to the day containing it.
    let days = millis.div_euclid(86_400_000);
    let within = millis.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    Some(DateParts {
        year,
        month,
        day,
        // 1970-01-01 was a Thursday, which is day 4.
        weekday: (days + 4).rem_euclid(7) as u32,
        hour: (within / 3_600_000) as u32,
        minute: (within / 60_000 % 60) as u32,
        second: (within / 1000 % 60) as u32,
        millisecond: (within % 1000) as u32,
    })
}

/// The instant a set of UTC parts names, with every part allowed out of range.
///
/// Out-of-range parts roll over rather than clamping: `setMonth(12)` is January of the next year
/// and `setDate(0)` is the last day of the previous month. That falls out of normalising the
/// month into a year offset and then counting in whole units, and it is what the specification
/// says -- clamping would quietly produce a different date than the program asked for.
fn parts_to_instant(parts: [f64; 7]) -> f64 {
    let [year, month, day, hour, minute, second, millisecond] = parts;
    if parts.iter().any(|part| !part.is_finite()) {
        return f64::NAN;
    }
    let months = month.floor();
    let year = year.trunc() + (months / 12.0).floor();
    let month = months.rem_euclid(12.0);
    // Beyond what a civil date can name; the calendar arithmetic below would overflow.
    if year.abs() > 400_000.0 {
        return f64::NAN;
    }
    let days = days_from_civil(year as i64, month as i64 + 1, 1) as f64 + (day - 1.0);
    days * 86_400_000.0 + hour * 3_600_000.0 + minute * 60_000.0 + second * 1000.0 + millisecond
}

/// `toISOString`, or `None` for an instant that has no ISO spelling.
fn iso_string(time: f64) -> Option<String> {
    let parts = date_parts(time)?;
    // A year outside four digits is written with a sign and six, which is the only shape that
    // stays unambiguous.
    let year = if (0..=9999).contains(&parts.year) {
        format!("{:04}", parts.year)
    } else {
        format!(
            "{}{:06}",
            if parts.year < 0 { '-' } else { '+' },
            parts.year.abs()
        )
    };
    Some(format!(
        "{year}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        parts.month, parts.day, parts.hour, parts.minute, parts.second, parts.millisecond
    ))
}

/// `String(date)`: the human form, which is UTC here because the realm has no other zone.
fn date_to_string(time: f64) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Some(parts) = date_parts(time) else {
        return "Invalid Date".to_owned();
    };
    format!(
        "{} {} {:02} {} {:02}:{:02}:{:02} GMT+0000 (Coordinated Universal Time)",
        WEEKDAYS[parts.weekday as usize],
        MONTHS[(parts.month - 1) as usize],
        parts.day,
        parts.year,
        parts.hour,
        parts.minute,
        parts.second
    )
}

/// The instant named by `(year, month, day, hours, minutes, seconds, milliseconds)`.
///
/// Out-of-range fields roll over rather than failing -- month 13 is January of the next year --
/// which is why the arithmetic is done in milliseconds rather than validated field by field.
fn date_from_fields(arguments: &[Value]) -> f64 {
    let field = |index: usize, default: f64| -> f64 {
        arguments.get(index).map_or(default, Value::as_number)
    };
    let year = field(0, f64::NAN);
    if !year.is_finite() {
        return f64::NAN;
    }
    // A two-digit year means 19xx, which is the one piece of legacy the constructor keeps.
    let year = if (0.0..=99.0).contains(&year) {
        year + 1900.0
    } else {
        year
    };
    let (month, day) = (field(1, 0.0), field(2, 1.0));
    let (hours, minutes) = (field(3, 0.0), field(4, 0.0));
    let (seconds, millis) = (field(5, 0.0), field(6, 0.0));
    if [month, day, hours, minutes, seconds, millis]
        .iter()
        .any(|value| !value.is_finite())
    {
        return f64::NAN;
    }
    // The month is zero-based and may sit outside 0..12, so it is folded into the year first.
    let months = year as i64 * 12 + month as i64;
    let days = days_from_civil(months.div_euclid(12), months.rem_euclid(12) + 1, day as i64);
    (days * 86_400_000) as f64
        + hours * 3_600_000.0
        + minutes * 60_000.0
        + seconds * 1000.0
        + millis
}

/// Parse the ISO forms `Date.parse` and the string constructor accept.
///
/// Deliberately only ISO 8601. The specification also allows an implementation-defined set of
/// other formats, and guessing at one silently turns a typo into a date.
fn parse_date(text: &str) -> f64 {
    let text = text.trim();
    let bytes = text.as_bytes();
    let read =
        |range: std::ops::Range<usize>| -> Option<i64> { text.get(range)?.parse::<i64>().ok() };
    if bytes.len() < 10 || bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
        return f64::NAN;
    }
    let (Some(year), Some(month), Some(day)) = (read(0..4), read(5..7), read(8..10)) else {
        return f64::NAN;
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return f64::NAN;
    }
    let mut millis = days_from_civil(year, month, day) * 86_400_000;
    if bytes.len() > 10 {
        if bytes.get(10) != Some(&b'T') || bytes.len() < 16 {
            return f64::NAN;
        }
        let (Some(hour), Some(minute)) = (read(11..13), read(14..16)) else {
            return f64::NAN;
        };
        millis += hour * 3_600_000 + minute * 60_000;
        if bytes.len() >= 19 && bytes.get(16) == Some(&b':') {
            let Some(second) = read(17..19) else {
                return f64::NAN;
            };
            millis += second * 1000;
            if bytes.len() >= 23 && bytes.get(19) == Some(&b'.') {
                let Some(fraction) = read(20..23) else {
                    return f64::NAN;
                };
                millis += fraction;
            }
        }
    }
    millis as f64
}

/// Whether a value can be called.
fn is_callable(value: &Value) -> bool {
    matches!(
        value,
        Value::Function(_)
            | Value::Native(_)
            | Value::Callback(_)
            | Value::Settler(..)
            | Value::Bound(_)
            | Value::Proxy(_)
    )
}

/// One match: where it landed, and where each capture group did.
///
/// Every range is in UTF-16 units, which is what the language counts in and what the engine
/// reports when matching over UTF-16.
type RegExpMatch = (
    std::ops::Range<usize>,
    Vec<Option<std::ops::Range<usize>>>,
    Vec<(String, Option<std::ops::Range<usize>>)>,
);

/// Where a literal needle occurs, in the shape a regexp match has, so replacement is one path.
fn literal_matches(subject: &[u16], needle: &[u16], all: bool) -> Vec<RegExpMatch> {
    let mut found = Vec::new();
    let mut at = 0;
    while at + needle.len() <= subject.len() {
        if subject[at..at + needle.len()] == *needle {
            let range = at..at + needle.len();
            found.push((range.clone(), vec![Some(range)], Vec::new()));
            if !all {
                break;
            }
            // An empty needle would never advance, so it steps one at a time.
            at = if needle.is_empty() {
                at + 1
            } else {
                at + needle.len()
            };
            continue;
        }
        at += 1;
    }
    found
}

/// Expand `$&`, `` $` ``, `$'`, `$1`..`$99` and `$$` in a replacement string.
fn expand_replacement(
    template: &str,
    subject: &[u16],
    range: &std::ops::Range<usize>,
    groups: &[Option<std::ops::Range<usize>>],
    named: &[(String, Option<std::ops::Range<usize>>)],
) -> String {
    let mut out = String::new();
    let characters: Vec<char> = template.chars().collect();
    let mut at = 0;
    while at < characters.len() {
        if characters[at] != '$' || at + 1 >= characters.len() {
            out.push(characters[at]);
            at += 1;
            continue;
        }
        match characters[at + 1] {
            '$' => {
                out.push('$');
                at += 2;
            }
            '&' => {
                out.push_str(&utf16_slice(subject, range.clone()));
                at += 2;
            }
            '`' => {
                out.push_str(&utf16_slice(subject, 0..range.start));
                at += 2;
            }
            '\'' => {
                out.push_str(&utf16_slice(subject, range.end..subject.len()));
                at += 2;
            }
            '<' => {
                let Some(end) = characters[at + 2..]
                    .iter()
                    .position(|character| *character == '>')
                else {
                    out.push('$');
                    at += 1;
                    continue;
                };
                let end = at + 2 + end;
                let name: String = characters[at + 2..end].iter().collect();
                if let Some((_, Some(group))) =
                    named.iter().find(|(candidate, _)| candidate == &name)
                {
                    out.push_str(&utf16_slice(subject, group.clone()));
                } else if named.is_empty() {
                    // A named replacement token is literal when this regexp has no named
                    // captures. With named captures, an unknown or unmatched name is empty.
                    out.push('$');
                    at += 1;
                    continue;
                }
                at = end + 1;
            }
            first if first.is_ascii_digit() => {
                // Two digits are preferred when that names a group, which is why `$12` is group
                // twelve where there are twelve and group one followed by "2" where there are not.
                let two = characters
                    .get(at + 2)
                    .filter(|second| second.is_ascii_digit())
                    .and_then(|second| format!("{first}{second}").parse::<usize>().ok())
                    .filter(|index| *index > 0 && *index < groups.len());
                let (index, width) = match two {
                    Some(index) => (index, 3),
                    None => match first.to_digit(10).map(|digit| digit as usize) {
                        Some(index) if index > 0 && index < groups.len() => (index, 2),
                        // Not a group reference at all, so the text stands as written.
                        _ => {
                            out.push('$');
                            at += 1;
                            continue;
                        }
                    },
                };
                if let Some(Some(group)) = groups.get(index) {
                    out.push_str(&utf16_slice(subject, group.clone()));
                }
                at += width;
            }
            _ => {
                out.push('$');
                at += 1;
            }
        }
    }
    out
}

/// Every match of a global pattern, from the start of the subject.
///
/// An empty match advances the cursor by one, or a pattern that can match nothing -- `/a*/g` --
/// would never finish.
fn regexp_search_all(regexp: &RegExpData, subject: &[u16]) -> Vec<RegExpMatch> {
    let mut found = Vec::new();
    let mut at = 0;
    while at <= subject.len() {
        let Some((range, groups, named)) = regexp_search(regexp, subject, at) else {
            break;
        };
        at = if range.end == range.start {
            range.end + 1
        } else {
            range.end
        };
        found.push((range, groups, named));
    }
    found
}

/// The array `exec` and `match` produce: the whole match, then each group, with `index` and
/// `input` alongside.
fn match_result(
    subject: &[u16],
    range: &std::ops::Range<usize>,
    groups: &[Option<std::ops::Range<usize>>],
    named: &[(String, Option<std::ops::Range<usize>>)],
) -> Value {
    let mut items = vec![Value::String(utf16_slice(subject, range.clone()))];
    // The first group `regress` reports is the whole match, which is already element zero.
    for group in groups.iter().skip(1) {
        items.push(match group {
            Some(range) => Value::String(utf16_slice(subject, range.clone())),
            // A group that did not participate is undefined rather than empty.
            None => Value::Undefined,
        });
    }
    // `index` and `input` are properties *of the array*, which is why an array needs somewhere
    // to keep a property that is not an element.
    let result = ArrayData::new(items);
    result
        .properties
        .borrow_mut()
        .set("index", Value::Number(range.start as f64));
    result.properties.borrow_mut().set(
        "input",
        Value::String(utf16_slice(subject, 0..subject.len())),
    );
    result.properties.borrow_mut().set(
        "groups",
        if named.is_empty() {
            Value::Undefined
        } else {
            named_groups_object(subject, named)
        },
    );
    Value::Array(result)
}

/// Build the `groups` object carried by a match. A fresh ordinary object has no prototype in the
/// interpreter, matching the specification's null-prototype groups object. Duplicate names have
/// already been collapsed by `regress` with its first-participating-group rule.
fn named_groups_object(
    subject: &[u16],
    named: &[(String, Option<std::ops::Range<usize>>)],
) -> Value {
    let object = Rc::new(RefCell::new(ObjectData::default()));
    object.borrow_mut().set_prototype(Value::Null);
    for (name, group) in named {
        let value = group.as_ref().map_or(Value::Undefined, |range| {
            Value::String(utf16_slice(subject, range.clone()))
        });
        object.borrow_mut().set(name, value);
    }
    Value::Object(object)
}

/// The text of a UTF-16 range, as a JavaScript string.
fn utf16_slice(subject: &[u16], range: std::ops::Range<usize>) -> String {
    String::from_utf16_lossy(&subject[range])
}

/// The methods on `RegExp.prototype`, without the per-instance data a regexp also answers to.
///
/// A program reaches for these to call one against a receiver of its own -- `RegExp.prototype
/// .test.call(re, s)` is how a library avoids a monkey-patched `test`.
fn regexp_method(name: &str) -> Value {
    match name {
        "test" => Value::Native(Native::RegExpTest),
        "exec" => Value::Native(Native::RegExpExec),
        "toString" => Value::Native(Native::RegExpToString),
        _ => Value::Undefined,
    }
}

/// The methods on `Date.prototype`, for the same reason.
fn date_method(name: &str) -> Value {
    match name {
        "getTime" | "valueOf" => Value::Native(Native::DateField(DateField::Time)),
        "toISOString" | "toJSON" => Value::Native(Native::DateToIso),
        "toString" => Value::Native(Native::DateToString),
        _ => Value::Undefined,
    }
}

/// What every object inherits from `Object.prototype`.
///
/// `toString` and `valueOf` are deliberately absent from the *fallback* an ordinary object gets:
/// `ToPrimitive` looks for a `toString` and would start finding this one, changing what `"" + {}`
/// and every comparison against an object produce. They are reachable through
/// `Object.prototype.toString`, which is where programs actually use them.
fn object_property(name: &str) -> Value {
    match name {
        "hasOwnProperty" => Value::Native(Native::ObjectHasOwnProperty),
        "isPrototypeOf" => Value::Native(Native::ObjectIsPrototypeOf),
        "propertyIsEnumerable" => Value::Native(Native::ObjectPropertyIsEnumerable),
        "toString" | "toLocaleString" => Value::Native(Native::ObjectToStringTag),
        "valueOf" => Value::Native(Native::ObjectValueOf),
        _ => Value::Undefined,
    }
}

/// What an ordinary object inherits from `Object.prototype`.
///
/// `toString` and `valueOf` are here now that `ToPrimitive` runs on the machine and tries them in
/// the specification's order: `String({})` finds `toString` and answers "[object Object]", and
/// `{} - 1` finds `valueOf`, gets the object back, and moves on to `toString`. While the
/// reduction was a pure function these two could not be here, because it would have found them
/// and had no way to call them.
fn inherited_object_property(name: &str) -> Value {
    match name {
        "hasOwnProperty"
        | "isPrototypeOf"
        | "propertyIsEnumerable"
        | "toString"
        | "toLocaleString"
        | "valueOf" => object_property(name),
        _ => Value::Undefined,
    }
}

/// The object `new f()` gives instances, created on first use.
///
/// Created lazily so an ordinary function never used as a constructor never allocates one, and
/// carrying `constructor` back to the function -- which is what makes `x.constructor === C` true
/// and what a console reads to print `C {}` rather than `{}`. The two point at each other, so a
/// class that is never collected keeps its prototype alive; a realm's constructors outlive the
/// program anyway.
fn prototype_object(closure: &Rc<Closure>) -> Value {
    let mut slot = closure.prototype.borrow_mut();
    if let Some(existing) = slot.as_ref() {
        return existing.clone();
    }
    let mut data = ObjectData::default();
    data.set("constructor", Value::Function(Rc::clone(closure)));
    let prototype = Value::Object(Rc::new(RefCell::new(data)));
    *slot = Some(prototype.clone());
    prototype
}

/// The elements a typed array's constructor or `set` was given.
fn typed_source(value: Option<&Value>) -> Vec<f64> {
    match value {
        Some(Value::Array(items)) => dense_items(&items.borrow())
            .iter()
            .map(Value::as_number)
            .collect(),
        Some(Value::TypedArray(view)) => (0..view.length)
            .map(|index| view.get(index).unwrap_or(0.0))
            .collect(),
        _ => Vec::new(),
    }
}

/// `start` and `end` from a pair of possibly-negative, possibly-absent bounds.
fn slice_bounds(arguments: &[Value], length: usize) -> (usize, usize) {
    let resolve = |value: Option<&Value>, fallback: usize| -> usize {
        match value {
            None | Some(Value::Undefined) => fallback,
            Some(value) => {
                let at = value.as_number();
                if !at.is_finite() {
                    return if at < 0.0 { 0 } else { length };
                }
                if at < 0.0 {
                    (length as f64 + at).max(0.0) as usize
                } else {
                    (at as usize).min(length)
                }
            }
        }
    };
    let start = resolve(arguments.first(), 0);
    let end = resolve(arguments.get(1), length);
    (start, end.max(start))
}

/// `new Uint8Array(...)` in each of its four shapes.
///
/// A length allocates; an array or another view copies; an `ArrayBuffer` *shares*, which is the
/// shape that makes two views over one buffer see each other's writes.
fn construct_typed_array(kind: TypedKind, arguments: &[Value]) -> Value {
    let width = kind.width();
    match arguments.first() {
        Some(Value::ArrayBuffer(bytes)) => {
            let total = bytes.borrow().len();
            let offset = arguments.get(1).map_or(0.0, Value::as_number);
            let offset = if offset.is_finite() && offset >= 0.0 {
                (offset as usize).min(total)
            } else {
                0
            };
            let length = match arguments.get(2) {
                None | Some(Value::Undefined) => total.saturating_sub(offset) / width,
                Some(value) => {
                    let length = value.as_number();
                    if length.is_finite() && length >= 0.0 {
                        (length as usize).min(total.saturating_sub(offset) / width)
                    } else {
                        0
                    }
                }
            };
            Value::TypedArray(Rc::new(TypedArrayData {
                kind,
                buffer: Rc::clone(bytes),
                offset,
                length,
            }))
        }
        Some(Value::Array(_) | Value::TypedArray(_)) => {
            let items = typed_source(arguments.first());
            let view = TypedArrayData {
                kind,
                buffer: Rc::new(RefCell::new(vec![0; items.len() * width])),
                offset: 0,
                length: items.len(),
            };
            for (index, item) in items.into_iter().enumerate() {
                view.set(index, item);
            }
            Value::TypedArray(Rc::new(view))
        }
        other => {
            let length = other.map_or(0.0, Value::as_number);
            let length = if length.is_finite() && length >= 0.0 {
                length as usize
            } else {
                0
            };
            Value::TypedArray(Rc::new(TypedArrayData {
                kind,
                buffer: Rc::new(RefCell::new(vec![0; length * width])),
                offset: 0,
                length,
            }))
        }
    }
}

/// Whether this value reduces to a primitive by calling one of its own methods.
fn is_object_like(value: &Value) -> bool {
    matches!(
        value,
        Value::Object(_) | Value::Function(_) | Value::Array(_)
    )
}

/// What a proxy ultimately wraps, following a chain of them.
///
/// For the operations that are defined on the *shape* of a value rather than on its properties
/// -- `Array.isArray`, `instanceof` -- and which therefore have no trap to consult.
fn unwrap_proxy(value: &Value) -> &Value {
    let mut current = value;
    for _ in 0..MAX_PROTOTYPE_DEPTH {
        match current {
            Value::Proxy(proxy) => current = &proxy.target,
            other => return other,
        }
    }
    current
}

/// The built-in that constructs values of this kind, for `x.constructor`.
///
/// `None` for the kinds that answer through an ordinary prototype chain -- a plain object and a
/// compiled function both carry `constructor` as a property, and inventing one here would shadow
/// what the program put there.
fn intrinsic_constructor(value: &Value) -> Option<Value> {
    let native = match value {
        Value::Date(_) => Native::DateConstructor,
        Value::RegExp(_) => Native::RegExpConstructor,
        Value::Promise(_) => Native::PromiseConstructor,
        Value::Array(_) => Native::ArrayConstructor,
        Value::String(_) => Native::StringConstructor,
        Value::Number(_) => Native::NumberConstructor,
        Value::Boolean(_) => Native::BooleanConstructor,
        Value::BigInt(_) => Native::BigIntConstructor,
        Value::Symbol(_) => Native::SymbolConstructor,
        _ => return None,
    };
    Some(Value::Native(native))
}

/// `call`, `apply`, and `bind`, which every callable has and nothing else does.
///
/// Returned unbound, like every other method here: the receiver is the function being called
/// through, and `CallMethod` supplies it.
fn function_property(name: &str) -> Value {
    match name {
        // Not the source text, which the VM never sees -- the same reason an error carries no
        // `stack`. What it does promise is that two different functions give two different
        // strings, which is what the `isPlainObject` check every library ships depends on.
        "toString" => Value::Native(Native::FunctionToString),
        "call" => Value::Native(Native::FunctionCall),
        "apply" => Value::Native(Native::FunctionApply),
        "bind" => Value::Native(Native::FunctionBind),
        // A function is an object, so it inherits `Object.prototype`'s methods too --
        // `Object.hasOwnProperty.call(x, k)` reads them off the *constructor*, which is how the
        // plain-object check every library ships is written.
        other => inherited_object_property(other),
    }
}

/// The own, program-visible keys of a value, in insertion order.
fn own_property_keys(target: &Value) -> Vec<PropertyKey> {
    let from_object = |data: &ObjectData| -> Vec<PropertyKey> {
        data.all_entries()
            .filter(|(key, _)| !matches!(key, PropertyKey::Internal(_)))
            .map(|(key, _)| key.clone())
            .collect()
    };
    match target {
        Value::Object(object) => from_object(&object.borrow()),
        Value::Function(closure) => from_object(&closure.properties.borrow()),
        Value::Array(items) => {
            let mut keys: Vec<PropertyKey> = items
                .borrow()
                .iter()
                .enumerate()
                .filter(|(_, item)| !matches!(item, Value::Native(Native::Hole)))
                .map(|(index, _)| PropertyKey::String(index.to_string()))
                .collect();
            keys.push(PropertyKey::String("length".to_owned()));
            keys.extend(from_object(&items.properties.borrow()));
            keys
        }
        _ => Vec::new(),
    }
}

/// Every own property slot, including non-enumerable ones, in property-key order.
fn own_property_entries(value: &Value) -> Vec<(PropertyKey, Property)> {
    let from_object = |data: &ObjectData| {
        data.all_entries()
            .filter(|(key, _)| !matches!(key, PropertyKey::Internal(_)))
            .map(|(key, property)| (key.clone(), property.clone()))
            .collect::<Vec<_>>()
    };
    match value {
        Value::Object(object) => from_object(&object.borrow()),
        Value::Function(closure) => from_object(&closure.properties.borrow()),
        Value::Array(items) => {
            let mut properties = items
                .borrow()
                .iter()
                .enumerate()
                .filter(|(_, item)| !matches!(item, Value::Native(Native::Hole)))
                .map(|(index, item)| {
                    (
                        PropertyKey::String(index.to_string()),
                        Property::Value(item.clone()),
                    )
                })
                .collect::<Vec<_>>();
            properties.push((
                PropertyKey::String("length".to_owned()),
                Property::Value(Value::Number(items.borrow().len() as f64)),
            ));
            properties.extend(from_object(&items.properties.borrow()));
            properties
        }
        Value::String(text) => text
            .chars()
            .enumerate()
            .map(|(index, character)| {
                (
                    PropertyKey::String(index.to_string()),
                    Property::Value(Value::String(character.to_string())),
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// One own property described as the object `Object.getOwnPropertyDescriptor` returns.
fn own_descriptor(target: &Value, key: &PropertyKey) -> Option<Value> {
    let intrinsic_array_length =
        matches!(target, Value::Array(_)) && *key == PropertyKey::String("length".to_owned());
    let frozen = match target {
        Value::Object(object) => object.borrow().is_frozen(),
        Value::Function(closure) => closure.properties.borrow().is_frozen(),
        Value::Array(items) => items.properties.borrow().is_frozen(),
        _ => false,
    };
    let property = match target {
        Value::Object(object) => object.borrow().get_own_property(key).cloned(),
        Value::Function(closure) => closure.properties.borrow().get_own_property(key).cloned(),
        Value::Array(items) => match string_key(key) {
            Some("length") => Some(Property::Value(Value::Number(items.borrow().len() as f64))),
            Some(name) => match name.parse::<usize>().ok() {
                Some(index) => match items.borrow().get(index) {
                    Some(Value::Native(Native::Hole)) | None => None,
                    Some(item) => Some(Property::Value(item.clone())),
                },
                None => items
                    .properties
                    .borrow()
                    .get_own_property(key.clone())
                    .cloned(),
            },
            None => items
                .properties
                .borrow()
                .get_own_property(key.clone())
                .cloned(),
        },
        _ => None,
    };
    let mut data = ObjectData::default();
    match property? {
        Property::Value(value) => {
            data.set("value", value);
            data.set("writable", Value::Boolean(!frozen));
        }
        Property::Accessor { get, set } => {
            data.set("get", get.unwrap_or(Value::Undefined));
            data.set("set", set.unwrap_or(Value::Undefined));
        }
    }
    data.set(
        "enumerable",
        Value::Boolean(own_enumerable(target, &key_value(key))),
    );
    data.set(
        "configurable",
        Value::Boolean(!frozen && !intrinsic_array_length),
    );
    Some(Value::Object(Rc::new(RefCell::new(data))))
}

/// `flat`: splice nested arrays into the result, down to `depth` levels.
fn flatten(items: &[Value], depth: usize) -> Vec<Value> {
    let mut out = Vec::new();
    for item in items {
        match item {
            // A hole vanishes rather than becoming undefined, which is the one place `flat`
            // differs from copying.
            Value::Native(Native::Hole) => {}
            Value::Array(nested) if depth > 0 => {
                out.extend(flatten(&nested.borrow(), depth - 1));
            }
            other => out.push(other.clone()),
        }
    }
    out
}

/// An array's elements with every hole replaced by `undefined`.
///
/// Whatever *reads* an element gets undefined, so anything that copies elements out in bulk --
/// spread, `Array.from`, an iterator -- has to do the same replacement a single read does, or
/// the marker escapes into a value.
fn dense_items(items: &[Value]) -> Vec<Value> {
    items
        .iter()
        .map(|item| {
            if matches!(item, Value::Native(Native::Hole)) {
                Value::Undefined
            } else {
                item.clone()
            }
        })
        .collect()
}

fn array_item_present(item: &Value) -> bool {
    !matches!(item, Value::Native(Native::Hole))
}

/// How a console reports a run of holes: `<1 empty item>` or `<3 empty items>`.
fn empty_items(count: usize) -> String {
    if count == 1 {
        "<1 empty item>".to_owned()
    } else {
        format!("<{count} empty items>")
    }
}

/// A property name as a console writes it: bare when it is a plain identifier, quoted otherwise.
///
/// `$` is not in the set, which looks like an oversight and is not: node's own rule is
/// `/^[a-zA-Z_][a-zA-Z_0-9]*$/`, so `{ a$b: 1 }` prints with its key quoted and so does `{ 3: 1 }`.
/// Matching node exactly is what lets `scripts/differential.sh` diff console output at all.
/// The name a console shows for a function the runtime supplies rather than the program.
///
/// Derived from the variant rather than tabulated. A table of two hundred names would be two
/// hundred chances to disagree with the property table that actually resolves the name, and it
/// would go stale one variant at a time. The variants are spelled `<Type><Method>`, so removing
/// the type and lowering the first letter recovers the JavaScript name --
/// `conformance/differential/builtin-names.js` checks that against node for every builtin the
/// corpus can reach.
/// The name a console shows for a runtime-supplied callable.
///
/// `Error(Type)` is the one variant whose payload names the function rather than its
/// implementation, and the payload is spelled `Type` where the language spells it `TypeError` --
/// so it is answered from the same table that binds the global, not from the variant. Every other
/// name is derived, for the reason [`builtin_js_name`] gives.
fn native_js_name(native: &Native) -> String {
    match native {
        Native::Error(kind) => kind.name().to_owned(),
        // The URI four are an acronym the variant spelling flattens: `DecodeUri` lowers to
        // `decodeUri`, and the language spells it `decodeURI`. A derivation cannot recover a case
        // distinction that the input does not carry, so these are named outright.
        // `SetTimeout` starts with a type name that is not its type: the derivation strips `Set`
        // -- the collection -- and answers `timeout`. A prefix that is a word in its own right is
        // the one thing a name derived from a variant cannot get right on its own.
        Native::SetTimeout => "setTimeout".to_owned(),
        Native::SetInterval => "setInterval".to_owned(),
        Native::EncodeUri => "encodeURI".to_owned(),
        Native::EncodeUriComponent => "encodeURIComponent".to_owned(),
        Native::DecodeUri => "decodeURI".to_owned(),
        Native::DecodeUriComponent => "decodeURIComponent".to_owned(),
        other => builtin_js_name(&format!("{other:?}")),
    }
}

/// The name a namespace shows, or `None` for a callable.
///
/// `Json` is spelled `JSON` in the language, which is the one place the variant's spelling and
/// the program's disagree -- and the reason this is a table of three rather than a derivation.
const fn namespace_name(native: &Native) -> Option<&'static str> {
    match native {
        Native::Math => Some("Math"),
        Native::Json => Some("JSON"),
        Native::Reflect => Some("Reflect"),
        _ => None,
    }
}

fn builtin_js_name(variant: &str) -> String {
    // A payload is not part of the name: `DateField(Time)` is `getTime`'s implementation, not a
    // function called `DateField(Time)`.
    let variant = variant.split('(').next().unwrap_or(variant);
    // A constructor is named for its type, not for the word "Constructor".
    if let Some(type_name) = variant.strip_suffix("Constructor")
        && !type_name.is_empty()
    {
        return type_name.to_owned();
    }
    // `Object.prototype.toString` is spelled for what it reads, not for what it is called.
    if variant == "ObjectToStringTag" {
        return "toString".to_owned();
    }
    // Longest first, so `ArrayBuffer` is not read as `Array` followed by `Buffer`.
    const TYPES: [&str; 26] = [
        "AsyncGenerator",
        "ArrayBuffer",
        "TypedArray",
        "Collection",
        "Generator",
        "Primitive",
        "Promise",
        "Boolean",
        "Reflect",
        "Console",
        "Process",
        "RegExp",
        "String",
        "Symbol",
        "Object",
        "Number",
        "BigInt",
        "Global",
        "Array",
        "Proxy",
        "Date",
        "Json",
        "Math",
        "Weak",
        "Map",
        "Set",
    ];
    let method = TYPES
        .iter()
        .find_map(|type_name| {
            variant
                .strip_prefix(type_name)
                .filter(|rest| !rest.is_empty())
        })
        .unwrap_or(variant);
    let mut characters = method.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

fn inspect_key<K: Into<PropertyKey>>(key: K) -> String {
    let key = key.into();
    let PropertyKey::String(key) = &key else {
        return match &key {
            PropertyKey::Symbol(symbol) => format!("[Symbol({})]", symbol.description()),
            PropertyKey::Internal(_) => "[Internal]".to_owned(),
            PropertyKey::String(_) => unreachable!(),
        };
    };
    let mut characters = key.chars();
    let identifier = characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|rest| rest.is_ascii_alphanumeric() || rest == '_');
    if identifier {
        key.to_owned()
    } else {
        quote_for_console(key)
    }
}

/// A string as a console shows it.
///
/// The quote character is whichever one the string does not already contain, so the common case
/// needs no escaping at all: `"it's"` rather than `'it\'s'`. Single, then double, then backtick,
/// which is node's order of preference.
fn quote_for_console(text: &str) -> String {
    let quote = if !text.contains('\'') {
        '\''
    } else if !text.contains('"') {
        '"'
    } else if !text.contains('`') && !text.contains("${") {
        '`'
    } else {
        '\''
    };

    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push(quote);
    for character in text.chars() {
        match character {
            other if other == quote => {
                quoted.push('\\');
                quoted.push(other);
            }
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '\u{8}' => quoted.push_str("\\b"),
            '\u{c}' => quoted.push_str("\\f"),
            control if control < ' ' => quoted.push_str(&format!("\\x{:02X}", control as u32)),
            other => quoted.push(other),
        }
    }
    quoted.push(quote);
    quoted
}

/// Marks the containers on the path from the root, so a cycle prints once instead of forever.
struct InspectGuard<'a> {
    seen: &'a mut Vec<usize>,
}

impl<'a> InspectGuard<'a> {
    fn enter(seen: &'a mut Vec<usize>, container: *const ()) -> Option<Self> {
        let identity = container as usize;
        if seen.contains(&identity) {
            return None;
        }
        seen.push(identity);
        Some(Self { seen })
    }
}

impl Drop for InspectGuard<'_> {
    fn drop(&mut self) {
        self.seen.pop();
    }
}

/// The numeric value of a property key that is an array index.
///
/// An index is a canonical decimal in `0 ..= 2^32 - 2`; `"01"`, `"1.0"`, `"-1"` and `"4294967295"`
/// are ordinary string keys. Checking the round trip rather than just parsing is what draws that
/// line, and the line is observable: only an index enumerates early.
fn string_index(key: &PropertyKey) -> Option<u32> {
    let PropertyKey::String(key) = key else {
        return None;
    };
    let index: u32 = key.parse().ok()?;
    (index != u32::MAX && index.to_string() == *key).then_some(index)
}

#[cfg(test)]
fn index_key(key: &str) -> Option<u32> {
    string_index(&PropertyKey::String(key.to_owned()))
}

fn strict_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
        (Value::Boolean(a), Value::Boolean(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        // Objects and functions compare by identity: `===` asks whether two references name the
        // same object, not whether their contents match. `Rc::ptr_eq` is that question exactly,
        // and it also avoids borrowing a `RefCell` that a caller may already hold mutably.
        (Value::Object(a), Value::Object(b)) => Rc::ptr_eq(a, b),
        (Value::Function(a), Value::Function(b)) => Rc::ptr_eq(a, b),
        // One `Error` per realm, so two references to it are the same function.
        (Value::Native(a), Value::Native(b)) => a == b,
        (Value::Callback(a), Value::Callback(b)) => a == b,
        (Value::Generator(a), Value::Generator(b)) => Rc::ptr_eq(a, b),
        (Value::Array(a), Value::Array(b)) => Rc::ptr_eq(a, b),
        // Every other object kind compares by identity for the same reason: each is one object,
        // and `x === x` has to hold for it. Leaving any of them out of this list makes a value
        // unequal to itself, which is the kind of wrong answer nothing recovers from -- a `Map`
        // keyed on one stops finding it, and a library checking whether a value is the same one
        // it stored always says no.
        (Value::Date(a), Value::Date(b)) => Rc::ptr_eq(a, b),
        (Value::RegExp(a), Value::RegExp(b)) => Rc::ptr_eq(a, b),
        (Value::Promise(a), Value::Promise(b)) => Rc::ptr_eq(a, b),
        (Value::Proxy(a), Value::Proxy(b)) => Rc::ptr_eq(a, b),
        (Value::TypedArray(a), Value::TypedArray(b)) => Rc::ptr_eq(a, b),
        (Value::ArrayBuffer(a), Value::ArrayBuffer(b)) => Rc::ptr_eq(a, b),
        (Value::Bound(a), Value::Bound(b)) => Rc::ptr_eq(a, b),
        (Value::Settler(a, one), Value::Settler(b, other)) => Rc::ptr_eq(a, b) && one == other,
        // A BigInt is a primitive: two of them are equal when the numbers are, however they were
        // built. That is the one entry here that is not an identity comparison.
        (Value::BigInt(a), Value::BigInt(b)) => a == b,
        (Value::Symbol(a), Value::Symbol(b)) => a == b,
        _ => false,
    }
}

/// `==`, the abstract equality comparison.
///
/// Not `strict_equal` with a coercion in front of it. The rules are their own algorithm and two
/// of them are not conversions at all: `null == undefined` is true while neither converts to the
/// other, and every comparison with NaN is false however it is spelled.
fn loose_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        // Same type: `==` is `===`, including NaN being unequal to itself.
        (Value::Undefined | Value::Null, Value::Undefined | Value::Null) => true,
        (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_))
        | (Value::Boolean(_), Value::Boolean(_))
        | (Value::Symbol(_), Value::Symbol(_)) => strict_equal(left, right),
        // Nullish compares equal to itself and to nothing else -- not even to 0 or "".
        (Value::Undefined | Value::Null, _) | (_, Value::Undefined | Value::Null) => false,
        // A boolean converts to a number *first*, so `[] == false` is true by way of "" and 0.
        (Value::Boolean(value), other) => loose_equal(&Value::Number(f64::from(*value)), other),
        (other, Value::Boolean(value)) => loose_equal(other, &Value::Number(f64::from(*value))),
        // A symbol converts to nothing, so it is equal only to itself and that case is above.
        (Value::Symbol(_), _) | (_, Value::Symbol(_)) => false,
        // `==` is the one place the two numeric types meet: it compares mathematical values, so
        // `1n == 1` is true where `1n === 1` is false. A non-integral number can never equal a
        // BigInt, and neither can a string that is not an integer.
        (Value::BigInt(big), Value::Number(number))
        | (Value::Number(number), Value::BigInt(big)) => {
            number.is_finite()
                && number.fract() == 0.0
                && num_bigint::BigInt::parse_bytes(format!("{number:.0}").as_bytes(), 10)
                    .is_some_and(|other| other == **big)
        }
        (Value::BigInt(big), Value::String(text)) | (Value::String(text), Value::BigInt(big)) => {
            text.trim()
                .parse::<num_bigint::BigInt>()
                .is_ok_and(|other| other == **big)
        }
        (Value::Number(number), Value::String(text))
        | (Value::String(text), Value::Number(number)) => {
            Value::String(text.clone()).as_number() == *number
        }
        // An object compares by identity against another object, and by its primitive form
        // against anything else -- which is what makes `[1] == 1` true.
        (Value::Object(_) | Value::Array(_), Value::Object(_) | Value::Array(_)) => {
            strict_equal(left, right)
        }
        (object @ (Value::Object(_) | Value::Array(_)), primitive) => {
            loose_equal(&to_primitive(object), primitive)
        }
        (primitive, object @ (Value::Object(_) | Value::Array(_))) => {
            loose_equal(primitive, &to_primitive(object))
        }
        _ => strict_equal(left, right),
    }
}

/// `ToPrimitive` with no hint, which for everything here means `toString`.
///
/// `valueOf` comes first in the specification and is skipped because nothing yet returns a
/// primitive from it; when a user-defined `valueOf` can exist, this needs the interpreter and
/// moves onto it.
fn to_primitive(value: &Value) -> Value {
    Value::String(value.to_display_string())
}

fn compare(left: &Value, right: &Value, predicate: OrderingPredicate) -> bool {
    if let (Value::String(left), Value::String(right)) = (left, right) {
        // ECMAScript compares strings by UTF-16 code units, rather than Unicode scalar values.
        let ordering = left.encode_utf16().cmp(right.encode_utf16());
        return match predicate {
            OrderingPredicate::Less => ordering.is_lt(),
            OrderingPredicate::LessEqual => ordering.is_le(),
            OrderingPredicate::Greater => ordering.is_gt(),
            OrderingPredicate::GreaterEqual => ordering.is_ge(),
        };
    }
    match predicate {
        OrderingPredicate::Less => left.as_number() < right.as_number(),
        OrderingPredicate::LessEqual => left.as_number() <= right.as_number(),
        OrderingPredicate::Greater => left.as_number() > right.as_number(),
        OrderingPredicate::GreaterEqual => left.as_number() >= right.as_number(),
    }
}

#[derive(Clone, Copy)]
enum OrderingPredicate {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

fn parse_number(value: &str) -> f64 {
    kiln_host_abi::string_to_number(value)
}

/// A number as `String(value)` renders it.
///
/// Delegates to the ABI crate rather than repeating the rules: both halves must agree about what
/// `String(1e21)` is, and the surest way for two copies to agree is for there to be one.
fn number_string(value: f64) -> String {
    kiln_host_abi::number_to_js_string(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_host_abi::{HostError, HostOp};

    struct TestHost {
        calls: Vec<(HostOp, Vec<HostValue>)>,
    }

    impl HostOps for TestHost {
        fn call(
            &mut self,
            operation: HostOp,
            arguments: &[HostValue],
        ) -> Result<HostValue, HostError> {
            self.calls.push((operation, arguments.to_vec()));
            Ok(HostValue::Undefined)
        }
    }

    /// `f(a, b)` compiled by hand: proto 0 returns its second parameter.
    fn calling_program(argc: u16, arguments: Vec<Instruction>) -> Program {
        let mut code = vec![Instruction::MakeFunction(0)];
        code.extend(arguments);
        code.push(Instruction::Call { argc });
        code.push(Instruction::Return);
        Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![number(1.0), number(2.0), number(3.0)],
            code,
            local_count: 0,
            functions: vec![kiln_ir::FunctionProto {
                asynchronous: false,
                name: Some("second".to_owned()),
                arrow: false,
                generator: false,
                async_generator: false,
                parameter_count: 2,
                local_count: 2,
                code: vec![Instruction::LoadLocal(1), Instruction::Return],
                upvalues: Vec::new(),
            }],
        }
    }

    /// A stand-in program for a closure that is never called.
    ///
    /// The prototype-chain tests build closures to compare identities, not to run: the table
    /// they would index is irrelevant and an empty one says so.
    fn no_program() -> Rc<Program> {
        Rc::new(Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: Vec::new(),
            code: Vec::new(),
            local_count: 0,
            functions: Vec::new(),
        })
    }

    fn run(program: &Program) -> Result<Value, ExecutionError> {
        let mut host = TestHost { calls: Vec::new() };
        execute(
            Rc::new(program.clone()),
            &mut host,
            ExecutionLimits::default(),
        )
        .map(|result| result.value)
    }

    /// An object with the given own properties, in order.
    fn object(pairs: &[(&str, Value)]) -> Value {
        let mut data = ObjectData::default();
        for (key, value) in pairs {
            data.set(key, value.clone());
        }
        Value::Object(Rc::new(RefCell::new(data)))
    }

    /// Re-assigning a key keeps its position: JavaScript enumerates string keys in insertion
    /// order, so a `set` that moved the key would change an observable property of the object.
    /// `cause` follows InstallErrorCause: the property exists when the options object *has* the
    /// key, even when its value is undefined. Testing presence rather than value is the whole
    /// point of that step, and copying the value would lose the distinction.
    /// `a["0"]` and `a[0]` name the same element. `a[-1]` and `a[1.5]` are ordinary string
    /// properties, and treating them as indices would silently move data.
    #[test]
    fn only_integer_like_keys_are_array_indices() {
        assert_eq!(array_index(&Value::Number(0.0)), Some(0));
        assert_eq!(array_index(&Value::String("2".to_owned())), Some(2));
        assert_eq!(array_index(&Value::Number(-1.0)), None);
        assert_eq!(array_index(&Value::Number(1.5)), None);
        assert_eq!(array_index(&Value::Number(f64::INFINITY)), None);
        assert_eq!(array_index(&Value::String("x".to_owned())), None);
    }

    /// `in` asks about presence, which is not the same question as reading and comparing to
    /// undefined: a property explicitly set to undefined is present.
    #[test]
    fn in_tests_presence_rather_than_value() {
        let object = object(&[("set_to_undefined", Value::Undefined)]);
        assert!(has_property(
            &object,
            &Value::String("set_to_undefined".to_owned())
        ));
        assert!(!has_property(&object, &Value::String("absent".to_owned())));
    }

    /// `Symbol.iterator` is a virtual protocol property, while the spelling `"@@iterator"` is an
    /// ordinary string property. An own string property may coexist with the protocol symbol.
    #[test]
    fn arrays_expose_the_same_properties_to_reads_and_to_in() {
        let items = Value::Array(ArrayData::new(vec![Value::Number(1.0)]));
        if let Value::Array(array) = &items {
            array
                .properties
                .borrow_mut()
                .set("@@iterator", Value::Number(7.0));
        }
        for key in ["join", "values", "length", "@@iterator"] {
            assert!(
                has_property(&items, &Value::String(key.to_owned())),
                "`{key} in array` must be true"
            );
        }
        let symbol = Value::Symbol(Symbol::WellKnown(WellKnown::Iterator));
        assert!(has_property(&items, &symbol));
        let realm = Realm::new();
        let mut host = TestHost { calls: Vec::new() };
        let vm = Vm::new(
            no_program(),
            Vec::new(),
            &mut host,
            ExecutionLimits::default(),
            &realm,
        );
        assert_eq!(
            vm.property_key(
                &items,
                &Value::String("@@iterator".to_owned()).to_property_key()
            ),
            Value::Number(7.0)
        );
        assert!(matches!(
            vm.property_key(&items, &symbol.to_property_key()),
            Value::Native(Native::ArrayValues)
        ));
        assert!(has_property(&items, &Value::Number(0.0)), "index 0 present");
        assert!(!has_property(&items, &Value::Number(1.0)), "index 1 absent");
    }

    /// A symbol's printable description does not turn it into a string key.
    #[test]
    fn a_symbol_key_is_not_its_display_string() {
        let symbol = Value::Symbol(Symbol::WellKnown(WellKnown::Iterator));
        assert_eq!(symbol.to_display_string(), "Symbol(Symbol.iterator)");
        assert_eq!(symbol.type_of(), "symbol");
    }

    /// `[].values` read twice must be the same function, which is what
    /// `[][Symbol.iterator] === [].values` asserts. Binding a receiver at read time would make
    /// each read a fresh value and quietly break it.
    #[test]
    fn reading_an_array_method_twice_yields_the_same_function() {
        assert!(strict_equal(
            &array_property("values"),
            &array_property("values")
        ));
        assert!(matches!(array_property("@@iterator"), Value::Undefined));
        assert!(strict_equal(
            &array_property("join"),
            &array_property("join")
        ));
        assert!(!strict_equal(
            &array_property("join"),
            &array_property("values")
        ));
    }

    /// `join` treats null and undefined as empty rather than stringifying them, which is the one
    /// place its output differs from mapping `String` over the elements.
    #[test]
    fn join_renders_nullish_elements_as_empty() {
        let items = [
            Value::Number(1.0),
            Value::Undefined,
            Value::Null,
            Value::String("x".to_owned()),
        ];
        assert_eq!(join(&items, ","), "1,,,x");
    }

    #[test]
    fn error_cause_is_installed_by_presence_not_by_value() {
        let with_cause = build_error(
            ErrorKind::Error,
            &[
                Value::String("m".to_owned()),
                object(&[("cause", Value::Undefined)]),
            ],
        );
        let Value::Object(data) = &with_cause else {
            panic!("construct_error returns an object");
        };
        assert_eq!(data.borrow().get("cause"), Some(Value::Undefined));

        let without = build_error(ErrorKind::Error, &[Value::String("m".to_owned())]);
        let Value::Object(data) = &without else {
            panic!("construct_error returns an object");
        };
        assert_eq!(data.borrow().get("cause"), None, "no options, no cause");
    }

    /// A cause is carried by identity: the error holds the same object the caller passed.
    #[test]
    fn error_cause_keeps_the_identity_of_the_value_given() {
        let inner = object(&[("marker", Value::Number(1.0))]);
        let outer = build_error(
            ErrorKind::Error,
            &[
                Value::String("outer".to_owned()),
                object(&[("cause", inner.clone())]),
            ],
        );
        let Value::Object(data) = &outer else {
            panic!("construct_error returns an object");
        };
        let cause = data.borrow().get("cause").expect("cause present");
        assert!(strict_equal(&cause, &inner), "cause is the same object");
    }

    /// An error with no message gets a bare header, not one with a trailing colon.
    #[test]
    fn an_empty_message_produces_a_bare_stack_header() {
        for arguments in [Vec::new(), vec![Value::Undefined]] {
            let Value::Object(data) = build_error(ErrorKind::Error, &arguments) else {
                panic!("construct_error returns an object");
            };
            assert_eq!(
                data.borrow().get("stack"),
                Some(Value::String("Error".to_owned()))
            );
            assert_eq!(
                data.borrow().get("message"),
                Some(Value::String(String::new()))
            );
        }
    }

    /// `typeof null` is "object" -- a specification bug old enough to be load-bearing, and
    /// distinct from the diagnostic name, which says "null" because that is more useful there.
    #[test]
    fn typeof_reports_the_language_tags_including_the_null_one() {
        assert_eq!(Value::Null.type_of(), "object");
        assert_eq!(Value::Null.type_name(), "null");
        assert_eq!(Value::Undefined.type_of(), "undefined");
        assert_eq!(Value::Number(1.0).type_of(), "number");
        assert_eq!(
            Value::Native(Native::Error(ErrorKind::Error)).type_of(),
            "function"
        );
        assert_eq!(object(&[]).type_of(), "object");
    }

    #[test]
    fn new_on_a_non_constructor_is_rejected() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![number(1.0)],
            code: vec![
                Instruction::Const(0),
                Instruction::New { argc: 0 },
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
        };
        assert!(matches!(
            run(&program),
            Err(ExecutionError::Uncaught { message, .. })
                if message == "TypeError: a number is not a constructor"
        ));
    }

    /// A write must create an own property rather than mutating the shared prototype, or two
    /// instances of one class would not be independent.
    /// `instanceof` compares the constructor's prototype by identity, walking the chain. Two
    /// unrelated classes with identical shapes must not be confused for one another.
    #[test]
    fn instanceof_walks_the_chain_by_identity() {
        let parent_prototype = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
        let parent = Rc::new(Closure {
            name: None,
            program: no_program(),
            captured_new_target: None,
            function: 0,
            upvalues: Vec::new(),
            captured_this: None,
            captured_arguments: None,
            prototype: RefCell::new(Some(parent_prototype.clone())),
            properties: RefCell::new(ObjectData::default()),
        });

        let mut child_prototype_data = ObjectData::default();
        child_prototype_data.set_prototype(parent_prototype);
        let child_prototype = Value::Object(Rc::new(RefCell::new(child_prototype_data)));
        let child = Rc::new(Closure {
            name: None,
            program: no_program(),
            captured_new_target: None,
            function: 1,
            upvalues: Vec::new(),
            captured_this: None,
            captured_arguments: None,
            prototype: RefCell::new(Some(child_prototype.clone())),
            properties: RefCell::new(ObjectData::default()),
        });

        let mut instance_data = ObjectData::default();
        instance_data.set_prototype(child_prototype);
        let instance = Value::Object(Rc::new(RefCell::new(instance_data)));

        assert!(instance_of(
            &instance,
            &Value::Function(Rc::clone(&child)),
            None
        ));
        assert!(
            instance_of(&instance, &Value::Function(Rc::clone(&parent)), None),
            "the chain is walked, not just the immediate link"
        );

        // A separate constructor with its own prototype must not match, however alike.
        let unrelated = Rc::new(Closure {
            name: None,
            program: no_program(),
            captured_new_target: None,
            function: 2,
            upvalues: Vec::new(),
            captured_this: None,
            captured_arguments: None,
            prototype: RefCell::new(Some(Value::Object(Rc::new(RefCell::new(
                ObjectData::default(),
            ))))),
            properties: RefCell::new(ObjectData::default()),
        });
        assert!(!instance_of(&instance, &Value::Function(unrelated), None));
        assert!(!instance_of(
            &Value::Number(1.0),
            &Value::Function(child),
            None
        ));
    }

    /// `String(e)` renders an error through its inherited `toString`, so an ordinary object that
    /// merely has `name` and `message` is left alone.
    #[test]
    fn error_to_string_is_keyed_on_the_inherited_method() {
        let plain = object(&[
            ("name", Value::String("NotAnError".to_owned())),
            ("message", Value::String("m".to_owned())),
        ]);
        assert!(!has_error_to_string(&plain));

        let erroneous = object(&[
            ("name", Value::String("E".to_owned())),
            ("message", Value::String("boom".to_owned())),
            ("toString", Value::Native(Native::ErrorToString)),
        ]);
        assert!(has_error_to_string(&erroneous));
        assert_eq!(error_to_string(&erroneous), "E: boom");
    }

    /// An error with no message renders as a bare name, with no trailing colon.
    #[test]
    fn error_to_string_omits_an_empty_message() {
        let bare = object(&[
            ("name", Value::String("Error".to_owned())),
            ("message", Value::String(String::new())),
        ]);
        assert_eq!(error_to_string(&bare), "Error");
    }

    /// Deleting during iteration must not skip the next element. Compacting the backing store
    /// shifts everything past the cursor by one, which is the bug this design exists to avoid.
    #[test]
    fn deleting_leaves_a_hole_rather_than_compacting() {
        let entries = vec![Value::Number(1.0), Value::Number(2.0), Value::Number(3.0)];
        assert_eq!(find_entry(&entries, &Value::Number(2.0), false), Some(1));

        let with_hole = vec![
            Value::Number(1.0),
            Value::Native(Native::Tombstone),
            Value::Number(3.0),
        ];
        assert_eq!(
            find_entry(&with_hole, &Value::Number(3.0), false),
            Some(2),
            "a hole must not shift the indices after it"
        );
        assert_eq!(
            find_entry(&with_hole, &Value::Number(2.0), false),
            None,
            "a deleted entry is not found"
        );
    }

    /// Set and Map use SameValueZero, which differs from `===` in exactly one place: NaN is its
    /// own key, so a Set can contain one and find it again.
    #[test]
    fn collections_key_on_same_value_zero() {
        let nan = Value::Number(f64::NAN);
        assert!(!strict_equal(&nan, &nan), "=== says NaN is not itself");
        assert!(same_value_zero(&nan, &nan), "SameValueZero says it is");

        assert!(same_value_zero(&Value::Number(0.0), &Value::Number(-0.0)));
        assert!(!same_value_zero(
            &Value::Number(1.0),
            &Value::String("1".to_owned())
        ));
    }

    /// A Map keyed by identity finds the same object and not an equal one.
    #[test]
    fn map_keys_are_matched_by_identity() {
        let key = object(&[("id", Value::Number(1.0))]);
        let twin = object(&[("id", Value::Number(1.0))]);
        let entries = vec![Value::Array(ArrayData::new(vec![
            key.clone(),
            Value::String("found".to_owned()),
        ]))];

        assert_eq!(find_entry(&entries, &key, true), Some(0));
        assert_eq!(
            find_entry(&entries, &twin, true),
            None,
            "an equal-looking object is a different key"
        );
    }

    /// An exception with no handler is an execution error naming what was thrown, rather than a
    /// silent stop or a panic.
    #[test]
    fn an_uncaught_throw_names_what_was_thrown() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![Constant::String("boom".to_owned())],
            code: vec![Instruction::Const(0), Instruction::Throw],
            local_count: 0,
            functions: Vec::new(),
        };
        let Err(ExecutionError::Uncaught { message, .. }) = run(&program) else {
            panic!("an unhandled throw must not succeed");
        };
        assert_eq!(message, "boom");
    }

    /// A handler restores the stack to the height it had when the handler was registered, so
    /// values left by the abandoned code cannot leak into the catch block.
    #[test]
    fn unwinding_restores_the_stack_the_handler_expected() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![number(1.0), number(2.0), Constant::String("x".to_owned())],
            code: vec![
                Instruction::Const(0), // a value the handler must still see
                Instruction::PushHandler(6),
                Instruction::Const(1), // debris left by the abandoned try block
                Instruction::Const(1),
                Instruction::Const(2),
                Instruction::Throw,
                // handler: the thrown value is on top, and the sentinel is beneath it
                Instruction::Pop,
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
        };
        assert_eq!(
            run(&program),
            Ok(Value::Number(1.0)),
            "the sentinel from before the handler survives, the debris does not"
        );
    }

    /// An error renders through its inherited `toString`; any other value through its ordinary
    /// string form, because JavaScript allows throwing anything.
    #[test]
    fn a_thrown_value_is_described_by_what_it_is() {
        let error = object(&[
            ("name", Value::String("E".to_owned())),
            ("message", Value::String("boom".to_owned())),
            ("toString", Value::Native(Native::ErrorToString)),
        ]);
        assert_eq!(describe_thrown(&error), "E: boom");
        assert_eq!(describe_thrown(&Value::Number(42.0)), "42");
        assert_eq!(describe_thrown(&Value::Undefined), "undefined");
    }

    #[test]
    fn a_write_does_not_reach_through_the_prototype() {
        let shared = Rc::new(RefCell::new(ObjectData::default()));
        shared.borrow_mut().set("value", Value::Number(1.0));

        let mut instance = ObjectData::default();
        instance.set_prototype(Value::Object(Rc::clone(&shared)));
        assert_eq!(instance.get("value"), Some(Value::Number(1.0)), "inherited");

        instance.set("value", Value::Number(2.0));
        assert_eq!(instance.get("value"), Some(Value::Number(2.0)), "own wins");
        assert_eq!(
            shared.borrow().get_own("value"),
            Some(&Value::Number(1.0)),
            "the prototype is untouched"
        );
    }

    /// Reads follow the chain to any depth; `get_own` never does. The two are separate because
    /// `in` and property assignment need opposite answers.
    #[test]
    fn reads_follow_the_prototype_chain_and_own_reads_do_not() {
        let grandparent = Rc::new(RefCell::new(ObjectData::default()));
        grandparent.borrow_mut().set("deep", Value::Number(3.0));

        let mut parent_data = ObjectData::default();
        parent_data.set_prototype(Value::Object(grandparent));
        let parent = Rc::new(RefCell::new(parent_data));

        let mut child = ObjectData::default();
        child.set_prototype(Value::Object(parent));

        assert_eq!(child.get("deep"), Some(Value::Number(3.0)));
        assert_eq!(child.get_own("deep"), None);
        assert_eq!(child.get("absent"), None);
    }

    #[test]
    fn reassigning_a_property_keeps_its_position() {
        let mut data = ObjectData::default();
        data.set("first", Value::Number(1.0));
        data.set("second", Value::Number(2.0));
        data.set("first", Value::Number(99.0));
        assert_eq!(data.keys().collect::<Vec<_>>(), ["first", "second"]);
        assert_eq!(data.get("first"), Some(Value::Number(99.0)));
    }

    /// `===` on objects asks whether two references name the same object. Structural equality
    /// would say two distinct objects with equal contents are the same, and then a write through
    /// one would appear to change the other.
    #[test]
    fn object_equality_is_identity_not_structure() {
        let left = object(&[("a", Value::Number(1.0))]);
        let right = object(&[("a", Value::Number(1.0))]);
        assert!(strict_equal(&left, &left), "an object equals itself");
        assert!(
            !strict_equal(&left, &right),
            "equal contents are not the same object"
        );
        assert!(strict_equal(&left, &left.clone()), "a clone shares the Rc");
    }

    /// A missing property is `undefined`, not an error: reading one is legal JavaScript.
    #[test]
    fn a_missing_property_reads_as_undefined() {
        let data = ObjectData::default();
        assert_eq!(data.get("absent"), None);
    }

    /// Assigning a property of a primitive is a TypeError in strict mode, and modules are always
    /// strict. Silently discarding it would lose a write the program believes it made.
    #[test]
    fn setting_a_property_on_a_primitive_is_rejected() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![Constant::String("x".to_owned()), number(1.0)],
            code: vec![
                Instruction::Const(1),
                Instruction::Const(1),
                Instruction::SetProperty(0),
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
        };
        assert!(matches!(
            run(&program),
            Err(ExecutionError::Uncaught { message, .. })
                if message == "TypeError: cannot set a property on a number"
        ));
    }

    #[test]
    fn closures_from_separate_calls_capture_separate_cells() {
        // outer(): let c = <cell>; return closure(inner) -- inner increments and returns c.
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![number(1.0)],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::Call { argc: 0 },
                Instruction::Call { argc: 0 },
                Instruction::MakeFunction(0),
                Instruction::Call { argc: 0 },
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            local_count: 0,
            functions: vec![
                kiln_ir::FunctionProto {
                    asynchronous: false,
                    name: Some("outer".to_owned()),
                    arrow: false,
                    generator: false,
                    async_generator: false,
                    parameter_count: 0,
                    local_count: 1,
                    code: vec![
                        Instruction::NewCell(0),
                        Instruction::Const(0),
                        Instruction::StoreCell(0),
                        Instruction::MakeFunction(1),
                        Instruction::Return,
                    ],
                    upvalues: Vec::new(),
                },
                kiln_ir::FunctionProto {
                    asynchronous: false,
                    name: Some("inner".to_owned()),
                    arrow: false,
                    generator: false,
                    async_generator: false,
                    parameter_count: 0,
                    local_count: 0,
                    code: vec![
                        Instruction::LoadUpvalue(0),
                        Instruction::Const(0),
                        Instruction::Binary(BinaryOp::Add),
                        Instruction::Dup,
                        Instruction::StoreUpvalue(0),
                        Instruction::Return,
                    ],
                    upvalues: vec![Upvalue::ParentLocal(0)],
                },
            ],
        };
        // Each outer() call makes a fresh cell seeded to 1, so each inner() returns 2.
        assert_eq!(run(&program), Ok(Value::Number(2.0)));
    }

    /// A closure keeps working after the call that created it has returned, which is the whole
    /// reason captures are cells rather than pointers into a frame.
    #[test]
    fn a_captured_cell_outlives_the_frame_that_created_it() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![number(5.0)],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::Call { argc: 0 },
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            local_count: 0,
            functions: vec![
                kiln_ir::FunctionProto {
                    asynchronous: false,
                    name: Some("outer".to_owned()),
                    arrow: false,
                    generator: false,
                    async_generator: false,
                    parameter_count: 0,
                    local_count: 1,
                    code: vec![
                        Instruction::NewCell(0),
                        Instruction::Const(0),
                        Instruction::StoreCell(0),
                        Instruction::MakeFunction(1),
                        Instruction::Return,
                    ],
                    upvalues: Vec::new(),
                },
                kiln_ir::FunctionProto {
                    asynchronous: false,
                    name: Some("read".to_owned()),
                    arrow: false,
                    generator: false,
                    async_generator: false,
                    parameter_count: 0,
                    local_count: 0,
                    code: vec![Instruction::LoadUpvalue(0), Instruction::Return],
                    upvalues: vec![Upvalue::ParentLocal(0)],
                },
            ],
        };
        assert_eq!(run(&program), Ok(Value::Number(5.0)));
    }

    #[test]
    fn capturing_a_slot_that_was_never_boxed_is_rejected() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![Instruction::MakeFunction(0), Instruction::Return],
            local_count: 1,
            functions: vec![kiln_ir::FunctionProto {
                asynchronous: false,
                name: None,
                arrow: false,
                generator: false,
                async_generator: false,
                parameter_count: 0,
                local_count: 0,
                code: vec![Instruction::Return],
                upvalues: vec![Upvalue::ParentLocal(0)],
            }],
        };
        assert!(matches!(
            run(&program),
            Err(ExecutionError::NotACell { slot: 0 })
        ));
    }

    #[test]
    fn a_call_binds_arguments_to_parameters_in_order() {
        let program = calling_program(2, vec![Instruction::Const(0), Instruction::Const(1)]);
        assert_eq!(run(&program), Ok(Value::Number(2.0)));
    }

    /// Wrong arity is not an error in JavaScript, so it is not one here either.
    #[test]
    fn missing_arguments_are_undefined_and_extra_ones_are_discarded() {
        let short = calling_program(1, vec![Instruction::Const(0)]);
        assert_eq!(
            run(&short),
            Ok(Value::Undefined),
            "second parameter unbound"
        );

        let long = calling_program(
            3,
            vec![
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::Const(2),
            ],
        );
        assert_eq!(run(&long), Ok(Value::Number(2.0)), "third argument dropped");
    }

    /// The caller's operand stack must survive the call untouched.
    #[test]
    fn a_call_leaves_only_its_result_on_the_caller_stack() {
        let mut program = calling_program(2, vec![Instruction::Const(0), Instruction::Const(1)]);
        // Push a sentinel first; it must still be underneath the result afterwards.
        program.code.insert(0, Instruction::Const(2));
        program.code.pop();
        program.code.push(Instruction::Pop);
        program.code.push(Instruction::Return);
        assert_eq!(run(&program), Ok(Value::Number(3.0)), "sentinel survived");
    }

    #[test]
    fn calling_a_non_function_is_an_error_rather_than_a_coercion() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![number(1.0)],
            code: vec![
                Instruction::Const(0),
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
        };
        // A TypeError the program could have caught, not a fatal stop: reaching the top with
        // no handler is what makes it an `Uncaught`, and the message names the kind.
        assert!(matches!(
            run(&program),
            Err(ExecutionError::Uncaught { message, .. })
                if message == "TypeError: a number is not a function"
        ));
    }

    /// Unbounded recursion has to be reported, not abort the process by exhausting the host
    /// stack. The step limit alone does not do it: frames are consumed far faster than steps.
    #[test]
    fn runaway_recursion_is_bounded_by_call_depth() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            local_count: 0,
            functions: vec![kiln_ir::FunctionProto {
                asynchronous: false,
                name: Some("forever".to_owned()),
                arrow: false,
                generator: false,
                async_generator: false,
                parameter_count: 0,
                local_count: 0,
                code: vec![
                    Instruction::MakeFunction(0),
                    Instruction::Call { argc: 0 },
                    Instruction::Return,
                ],
                upvalues: Vec::new(),
            }],
        };
        let mut host = TestHost { calls: Vec::new() };
        let limits = ExecutionLimits {
            max_call_depth: 8,
            ..ExecutionLimits::default()
        };
        // Reported as a thrown `RangeError`, which nothing here catches, rather than as a
        // fatal status: the bound is still enforced, and a program that wants to survive its own
        // recursion can now write the `try` that does it.
        let outcome = execute(Rc::new(program.clone()), &mut host, limits);
        assert!(
            matches!(&outcome, Err(ExecutionError::Uncaught { message, .. })
                if message.contains("Maximum call stack size exceeded")),
            "expected an uncaught RangeError, got {outcome:?}"
        );
    }

    /// A module slot written inside a frame must be visible to the next frame that reads it:
    /// that is the difference between module scope and a per-frame copy.
    #[test]
    fn module_slots_are_shared_across_frames() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![number(7.0)],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::Call { argc: 0 },
                Instruction::Pop,
                Instruction::MakeFunction(1),
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            local_count: 1,
            functions: vec![
                kiln_ir::FunctionProto {
                    asynchronous: false,
                    name: Some("write".to_owned()),
                    arrow: false,
                    generator: false,
                    async_generator: false,
                    parameter_count: 0,
                    local_count: 0,
                    code: vec![
                        Instruction::Const(0),
                        Instruction::StoreGlobal(0),
                        Instruction::Const(0),
                        Instruction::Return,
                    ],
                    upvalues: Vec::new(),
                },
                kiln_ir::FunctionProto {
                    asynchronous: false,
                    name: Some("read".to_owned()),
                    arrow: false,
                    generator: false,
                    async_generator: false,
                    parameter_count: 0,
                    local_count: 0,
                    code: vec![Instruction::LoadGlobal(0), Instruction::Return],
                    upvalues: Vec::new(),
                },
            ],
        };
        assert_eq!(run(&program), Ok(Value::Number(7.0)));
    }

    #[test]
    fn an_unknown_function_index_is_rejected_rather_than_indexed() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![Instruction::MakeFunction(3), Instruction::Return],
            local_count: 0,
            functions: Vec::new(),
        };
        assert!(matches!(
            run(&program),
            Err(ExecutionError::UnknownFunction { function: 3 })
        ));
    }

    #[test]
    fn private_accessor_reads_call_getter_and_private_in_checks_slot() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![Constant::String("@@private:0:value".into()), number(7.0)],
            code: vec![
                Instruction::NewObject { count: 0 },
                Instruction::MakeFunction(0),
                Instruction::DefinePrivateAccessor {
                    name: 0,
                    setter: false,
                },
                Instruction::Dup,
                Instruction::PrivateIn(0),
                Instruction::Pop,
                Instruction::GetPrivate(0),
                Instruction::Return,
            ],
            local_count: 0,
            functions: vec![kiln_ir::FunctionProto {
                asynchronous: false,
                name: Some("get value".into()),
                arrow: false,
                generator: false,
                async_generator: false,
                parameter_count: 0,
                local_count: 0,
                code: vec![Instruction::Const(1), Instruction::Return],
                upvalues: Vec::new(),
            }],
        };
        assert_eq!(run(&program), Ok(Value::Number(7.0)));
    }

    #[test]
    fn private_accessor_without_setter_rejects_writes_in_module_semantics() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![Constant::String("@@private:0:value".into()), number(2.0)],
            code: vec![
                Instruction::NewObject { count: 0 },
                Instruction::MakeFunction(0),
                Instruction::DefinePrivateAccessor {
                    name: 0,
                    setter: false,
                },
                Instruction::Const(1),
                Instruction::SetPrivate(0),
                Instruction::Return,
            ],
            local_count: 0,
            functions: vec![kiln_ir::FunctionProto {
                asynchronous: false,
                name: Some("get value".into()),
                arrow: false,
                generator: false,
                async_generator: false,
                parameter_count: 0,
                local_count: 0,
                code: vec![Instruction::Const(1), Instruction::Return],
                upvalues: Vec::new(),
            }],
        };
        assert!(matches!(
            run(&program),
            Err(ExecutionError::Uncaught { message, .. }) if message.contains("no setter")
        ));
    }

    #[test]
    fn private_slots_cover_array_targets_and_ignore_freeze() {
        let target = Value::Array(ArrayData::new(Vec::new()));
        let realm = Realm::new();
        let mut host = TestHost { calls: Vec::new() };
        let vm = Vm::new(
            no_program(),
            Vec::new(),
            &mut host,
            ExecutionLimits::default(),
            &realm,
        );
        let key = internal_key("@@private:0:value");
        assert!(private_target(&target));
        assert!(!vm.private_has(&target, &key));
        if let Value::Array(array) = &target {
            array.properties.borrow_mut().freeze();
        }
        assert!(vm.init_private(&target, key.clone(), Value::Number(3.0)));
        assert!(vm.private_has(&target, &key));
        assert_eq!(
            vm.private_property(&target, &key),
            Some(Property::Value(Value::Number(3.0)))
        );
        assert!(!private_target(&Value::BigInt(Rc::new(
            "1".parse().expect("valid bigint"),
        ))));
    }

    #[test]
    fn private_in_rejects_bigint_as_a_primitive_target() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![
                Constant::BigInt("1".into()),
                Constant::String("@@private:0:x".into()),
            ],
            code: vec![
                Instruction::Const(0),
                Instruction::PrivateIn(1),
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
        };
        assert!(matches!(
            run(&program),
            Err(ExecutionError::Uncaught { message, .. })
                if message.contains("right-hand side of `#x in` must be an object")
        ));
    }

    fn number(value: f64) -> Constant {
        Constant::Number(value.to_bits())
    }

    fn program(constants: Vec<Constant>, code: Vec<Instruction>) -> Program {
        Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants,
            code,
            local_count: 0,
            functions: Vec::new(),
        }
    }

    #[test]
    fn scalar_coercions_match_the_execution_slice() {
        assert!(!Value::Number(0.0).is_truthy());
        assert!(!Value::Number(f64::NAN).is_truthy());
        assert_eq!(Value::String("  ".into()).as_number(), 0.0);
        assert!(Value::String("x".into()).as_number().is_nan());
        assert_eq!(Value::Number(-0.0).to_display_string(), "0");
    }

    #[test]
    fn arithmetic_and_concat_are_scalar_operations() {
        assert_eq!(
            binary(BinaryOp::Add, Value::Number(2.0), Value::Number(3.0)),
            Value::Number(5.0)
        );
        assert_eq!(
            binary(BinaryOp::Add, Value::String("x".into()), Value::Number(3.0)),
            Value::String("x3".into())
        );
        assert_eq!(
            binary(
                BinaryOp::Exponentiate,
                Value::Number(2.0),
                Value::Number(3.0)
            ),
            Value::Number(8.0)
        );
        assert_eq!(
            binary(
                BinaryOp::StrictEqual,
                Value::Number(f64::NAN),
                Value::Number(f64::NAN)
            ),
            Value::Boolean(false)
        );
    }

    #[test]
    fn executes_arithmetic_and_reports_instruction_count() {
        let program = program(
            vec![number(2.0), number(3.0)],
            vec![
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::Binary(BinaryOp::Add),
                Instruction::Return,
            ],
        );
        let mut host = TestHost { calls: Vec::new() };
        let result = execute(
            Rc::new(program.clone()),
            &mut host,
            ExecutionLimits::default(),
        )
        .unwrap();
        assert_eq!(result.value, Value::Number(5.0));
        assert_eq!(result.steps, 4);
    }

    #[test]
    fn conditional_jumps_consume_conditions() {
        let program = program(
            vec![Constant::Boolean(false), number(1.0), number(2.0)],
            vec![
                Instruction::Const(0),
                Instruction::JumpIfFalse(4),
                Instruction::Const(1),
                Instruction::Jump(5),
                Instruction::Const(2),
                Instruction::Return,
            ],
        );
        let mut host = TestHost { calls: Vec::new() };
        let result = execute(
            Rc::new(program.clone()),
            &mut host,
            ExecutionLimits::default(),
        )
        .unwrap();
        assert_eq!(result.value, Value::Number(2.0));
    }

    #[test]
    fn nullish_jump_keeps_non_nullish_left_operand() {
        // This is the shape emitted for `left ?? right`: duplicate the left value, consume the
        // duplicate for the branch, and leave the selected operand on the stack.
        let program = program(
            vec![
                Constant::String("left".into()),
                Constant::String("right".into()),
            ],
            vec![
                Instruction::Const(0),
                Instruction::Dup,
                Instruction::JumpIfNotNullish(6),
                Instruction::Pop,
                Instruction::Const(1),
                Instruction::Jump(6),
                Instruction::Return,
            ],
        );
        let mut host = TestHost { calls: Vec::new() };
        let result = execute(
            Rc::new(program.clone()),
            &mut host,
            ExecutionLimits::default(),
        )
        .unwrap();
        assert_eq!(result.value, Value::String("left".into()));
    }

    #[test]
    fn host_calls_preserve_argument_order_and_round_trip_values() {
        let program = program(
            vec![Constant::String("first".into()), number(2.0)],
            vec![
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 2,
                },
                Instruction::Return,
            ],
        );
        let mut host = TestHost { calls: Vec::new() };
        execute(
            Rc::new(program.clone()),
            &mut host,
            ExecutionLimits::default(),
        )
        .unwrap();
        assert_eq!(host.calls.len(), 1);
        assert_eq!(host.calls[0].0, HostOp::WriteStdout);
        assert_eq!(host.calls[0].1[0], HostValue::String("first".into()));
        assert_eq!(host.calls[0].1[1], HostValue::number(2.0));
    }

    #[test]
    fn step_limit_stops_an_infinite_program() {
        let program = program(Vec::new(), vec![Instruction::Jump(0)]);
        let mut host = TestHost { calls: Vec::new() };
        let error = execute(
            Rc::new(program.clone()),
            &mut host,
            ExecutionLimits {
                max_steps: 3,
                ..ExecutionLimits::default()
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            ExecutionError::StepLimitExceeded { steps: 3, limit: 3 }
        );
    }

    #[test]
    fn module_execution_initializes_import_slots_and_publishes_exports() {
        let module = ModuleProgram {
            program: Program {
                version: kiln_ir::BYTECODE_VERSION,
                constants: vec![number(2.0)],
                // An import slot holds a cell and an exported slot must too, which is what the
                // compiler emits for both -- so the fixture reads through one and writes into
                // one, as real bytecode does.
                code: vec![
                    Instruction::NewCell(1),
                    Instruction::LoadCell(0),
                    Instruction::Const(0),
                    Instruction::Binary(BinaryOp::Multiply),
                    Instruction::StoreCell(1),
                    Instruction::Const(0),
                    Instruction::Return,
                ],
                local_count: 2,
                functions: Vec::new(),
            },
            dependencies: vec!["./dep.js".into()],
            imports: vec![kiln_ir::ImportBinding {
                source: "./dep.js".into(),
                imported: "value".into(),
                local: "input".into(),
                local_slot: 0,
            }],
            exports: vec![kiln_ir::ExportBinding {
                exported: "answer".into(),
                local_slot: 1,
            }],
        };
        let mut host = TestHost { calls: Vec::new() };

        let result = execute_module(
            &module,
            &[Value::Number(21.0)],
            &mut host,
            ExecutionLimits::default(),
        )
        .expect("module executes");

        assert_eq!(result.exports[0].name, "answer");
        // The cell, not its contents: an import is a live view of the exporter's binding, so
        // what is published is the box rather than what was in it when the module finished.
        assert_eq!(
            read_through_cell(&result.exports[0].value),
            Value::Number(42.0)
        );
    }

    #[test]
    fn standalone_reports_unhandled_rejections_in_creation_order() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![
                Constant::String("first".to_owned()),
                Constant::String("second".to_owned()),
                Constant::String("reject".to_owned()),
            ],
            code: vec![
                Instruction::LoadBuiltin(kiln_ir::Builtin::Promise),
                Instruction::Const(0),
                Instruction::CallMethod { name: 2, argc: 1 },
                Instruction::Pop,
                Instruction::LoadBuiltin(kiln_ir::Builtin::Promise),
                Instruction::Const(1),
                Instruction::CallMethod { name: 2, argc: 1 },
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
        };
        let error = run(&program).expect_err("an unhandled rejection should fail execution");
        assert!(matches!(
            error,
            ExecutionError::Uncaught { ref message, .. } if message.contains("first")
        ));
    }

    /// `json_stringify` for a fixture with no accessors, so it needs no interpreter to read one.
    fn serialize(value: &Value, indent: &str) -> Result<Option<String>, ExecutionError> {
        let mut unreachable_getter = |getter: Option<&Value>, receiver: &Value| match getter {
            // No fixture here defines an accessor, and none carries `toJSON`.
            Some(_) => unreachable!("no fixture here defines an accessor"),
            None => Ok(receiver.clone()),
        };
        json_stringify(value, indent, "", &mut Vec::new(), &mut unreachable_getter)
    }

    fn json(value: &Value) -> Option<String> {
        serialize(value, "").expect("no cycle in this fixture")
    }

    fn array(items: &[Value]) -> Value {
        Value::Array(ArrayData::new(items.to_vec()))
    }

    #[test]
    fn json_scalars_serialise_as_themselves() {
        assert_eq!(json(&Value::Null).as_deref(), Some("null"));
        assert_eq!(json(&Value::Boolean(true)).as_deref(), Some("true"));
        assert_eq!(json(&Value::Number(1.5)).as_deref(), Some("1.5"));
        assert_eq!(
            json(&Value::String("hi".to_owned())).as_deref(),
            Some(r#""hi""#)
        );
    }

    #[test]
    fn json_omits_what_it_cannot_spell() {
        // The caller turns `None` into `undefined`, a dropped property, or a null element --
        // three different outcomes that all start here.
        assert_eq!(json(&Value::Undefined), None);
        assert_eq!(
            json(&Value::Symbol(Symbol::WellKnown(WellKnown::Iterator))),
            None
        );
        assert_eq!(json(&Value::Native(Native::ArrayIsArray)), None);
    }

    #[test]
    fn json_writes_non_finite_numbers_as_null() {
        assert_eq!(json(&Value::Number(f64::NAN)).as_deref(), Some("null"));
        assert_eq!(json(&Value::Number(f64::INFINITY)).as_deref(), Some("null"));
    }

    #[test]
    fn json_drops_unrepresentable_properties_but_nulls_unrepresentable_elements() {
        // Dropping an array element would renumber every element after it, so the two containers
        // have to disagree here.
        let value = object(&[
            ("kept", Value::Number(1.0)),
            ("dropped", Value::Undefined),
            ("items", array(&[Value::Number(1.0), Value::Undefined])),
        ]);
        assert_eq!(
            json(&value).as_deref(),
            Some(r#"{"kept":1,"items":[1,null]}"#)
        );
    }

    #[test]
    fn json_skips_internal_properties_but_keeps_at_at_strings() {
        let mut data = ObjectData::default();
        data.set(internal_key("index"), Value::Number(0.0));
        data.set("@@index", Value::Number(1.0));
        data.set("real", Value::Number(2.0));
        let value = Value::Object(Rc::new(RefCell::new(data)));
        assert_eq!(json(&value).as_deref(), Some(r#"{"@@index":1,"real":2}"#));
    }

    #[test]
    fn json_escapes_strings() {
        let value = Value::String("a\"b\\c\nd\u{1}".to_owned());
        assert_eq!(json(&value).as_deref(), Some(r#""a\"b\\c\nd\u0001""#));
    }

    #[test]
    fn json_empty_containers_stay_on_one_line_when_indented() {
        let value = object(&[("a", array(&[])), ("b", object(&[]))]);
        let text = serialize(&value, "  ").expect("no cycle");
        assert_eq!(text.as_deref(), Some("{\n  \"a\": [],\n  \"b\": {}\n}"));
    }

    #[test]
    fn json_indents_nested_containers() {
        let value = object(&[("a", array(&[Value::Number(1.0)]))]);
        let text = serialize(&value, "  ").expect("no cycle");
        assert_eq!(text.as_deref(), Some("{\n  \"a\": [\n    1\n  ]\n}"));
    }

    #[test]
    fn json_rejects_a_value_that_contains_itself() {
        let items = ArrayData::new(Vec::new());
        items.borrow_mut().push(Value::Array(Rc::clone(&items)));
        assert!(matches!(
            serialize(&Value::Array(items), ""),
            Err(ExecutionError::CyclicJson)
        ));
    }

    #[test]
    fn json_allows_the_same_value_twice_in_different_places() {
        // A diamond is not a cycle. The guard tracks the path from the root, not every value it
        // has ever seen, which is the whole reason it pops on the way back out.
        let shared = object(&[("n", Value::Number(1.0))]);
        let value = array(&[shared.clone(), shared]);
        assert_eq!(json(&value).as_deref(), Some(r#"[{"n":1},{"n":1}]"#));
    }

    #[test]
    fn json_indent_argument_is_capped() {
        assert_eq!(json_indent(Some(&Value::Number(2.0))), "  ");
        assert_eq!(json_indent(Some(&Value::Number(50.0))).len(), 10);
        assert_eq!(json_indent(Some(&Value::String("ab".to_owned()))), "ab");
        // A fractional or zero count means no indent at all, same as omitting the argument.
        assert_eq!(json_indent(Some(&Value::Number(0.0))), "");
        assert_eq!(json_indent(None), "");
    }

    #[test]
    fn two_symbols_with_one_description_key_different_properties() {
        let first = Symbol::Unique(Rc::new(UniqueSymbol {
            serial: 1,
            description: Some("same".to_owned()),
        }));
        let second = Symbol::Unique(Rc::new(UniqueSymbol {
            serial: 2,
            description: Some("same".to_owned()),
        }));
        assert_ne!(first.property_key(), second.property_key());
        assert_ne!(first, second);
        assert_eq!(first, first.clone());
        // The description is for display only, and never reaches the key.
        assert_eq!(
            Value::Symbol(first).to_display_string(),
            "Symbol(same)".to_owned()
        );
    }

    #[test]
    fn symbols_are_typed_and_at_at_strings_are_not_symbols() {
        let symbol = Symbol::Unique(Rc::new(UniqueSymbol {
            serial: 7,
            description: None,
        }));
        let mut data = ObjectData::default();
        data.set(symbol.clone(), Value::Number(1.0));
        data.set("@@symbol:7", Value::Number(2.0));
        assert_eq!(data.get(symbol.clone()), Some(Value::Number(1.0)));
        assert_eq!(data.get("@@symbol:7"), Some(Value::Number(2.0)));
        assert_eq!(
            data.own_keys()
                .filter(|key| matches!(key, PropertyKey::Symbol(_)))
                .count(),
            1
        );
    }

    #[test]
    fn spread_reads_every_own_property_including_symbol_keyed_ones() {
        let symbol = Symbol::Unique(Rc::new(UniqueSymbol {
            serial: 1,
            description: None,
        }));
        let source = object(&[("plain", Value::Number(1.0))]);
        if let Value::Object(source) = &source {
            source.borrow_mut().set(symbol.clone(), Value::Number(2.0));
        }
        let copied = own_properties(&source);
        assert_eq!(copied.len(), 2, "a spread carries symbol keys across");
        assert_eq!(copied[0].0, "plain");
    }

    #[test]
    fn symbol_identity_survives_symbols_api_accessors_copies_and_delete() {
        let symbol = Symbol::Unique(Rc::new(UniqueSymbol {
            serial: 42,
            description: Some("state".to_owned()),
        }));
        let mut source = ObjectData::default();
        source.set(symbol.clone(), Value::Number(9.0));
        source.define_accessor(
            Symbol::WellKnown(WellKnown::ToStringTag),
            Some(Value::Native(Native::ArrayValues)),
            None,
        );
        let source = Value::Object(Rc::new(RefCell::new(source)));
        let symbols = own_property_keys(&source)
            .into_iter()
            .filter_map(|key| match key {
                PropertyKey::Symbol(symbol) => Some(symbol),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            symbols,
            vec![symbol.clone(), Symbol::WellKnown(WellKnown::ToStringTag)]
        );
        let copied = own_properties(&source);
        assert!(copied.iter().any(|(key, property)| {
            key == &PropertyKey::Symbol(symbol.clone())
                && matches!(property, Property::Value(Value::Number(9.0)))
        }));
        assert!(copied.iter().any(|(key, property)| {
            matches!(
                key,
                PropertyKey::Symbol(Symbol::WellKnown(WellKnown::ToStringTag))
            ) && matches!(property, Property::Accessor { .. })
        }));
        assert!(has_property(&source, &Value::Symbol(symbol.clone())));
        assert!(delete_property(&source, &Value::Symbol(symbol.clone())));
        assert!(!has_property(&source, &Value::Symbol(symbol)));
    }

    #[test]
    fn symbols_are_omitted_from_string_enumeration_and_json() {
        let symbol = Symbol::WellKnown(WellKnown::Iterator);
        let mut data = ObjectData::default();
        data.set(symbol, Value::Number(1.0));
        data.set("@@iterator", Value::Number(2.0));
        let value = Value::Object(Rc::new(RefCell::new(data)));
        assert_eq!(
            own_property_keys(&value)
                .into_iter()
                .filter_map(|key| string_key(&key).map(str::to_owned))
                .collect::<Vec<_>>(),
            vec!["@@iterator"]
        );
        assert_eq!(json(&value).as_deref(), Some(r#"{"@@iterator":2}"#));
    }

    #[test]
    fn arrays_keep_symbol_accessors_separate_from_string_properties() {
        let symbol = Symbol::Unique(Rc::new(UniqueSymbol {
            serial: 99,
            description: None,
        }));
        let array = ArrayData::new(Vec::new());
        array.properties.borrow_mut().define_accessor(
            symbol.clone(),
            Some(Value::Native(Native::ArrayValues)),
            None,
        );
        array
            .properties
            .borrow_mut()
            .set("@@symbol:99", Value::Number(3.0));
        let value = Value::Array(array);
        assert!(has_property(&value, &Value::Symbol(symbol.clone())));
        assert!(has_property(
            &value,
            &Value::String("@@symbol:99".to_owned())
        ));
        let realm = Realm::new();
        let mut host = TestHost { calls: Vec::new() };
        let vm = Vm::new(
            no_program(),
            Vec::new(),
            &mut host,
            ExecutionLimits::default(),
            &realm,
        );
        assert_eq!(
            vm.property_key(
                &value,
                &Value::String("@@symbol:99".to_owned()).to_property_key()
            ),
            Value::Number(3.0)
        );
        assert!(matches!(
            own_properties(&value)
                .into_iter()
                .find(|(key, _)| key == &PropertyKey::Symbol(symbol.clone())),
            Some((_, Property::Accessor { .. }))
        ));
    }

    #[test]
    fn array_own_property_apis_include_intrinsic_length() {
        let value = Value::Array(ArrayData::new(vec![Value::Number(1.0)]));
        assert!(own_property_exists(&value, &Value::String("length".into())));
        assert_eq!(
            own_property_keys(&value),
            vec![
                PropertyKey::String("0".into()),
                PropertyKey::String("length".into())
            ]
        );
        let descriptor = own_descriptor(&value, &PropertyKey::String("length".into()))
            .expect("array length descriptor");
        assert!(matches!(
            descriptor,
            Value::Object(object)
                if object.borrow().get("configurable") == Some(Value::Boolean(false))
        ));
    }

    #[test]
    fn array_index_descriptor_attributes_cover_enumeration_and_partial_updates() {
        let target = Value::Array(ArrayData::new(vec![Value::Number(7.0), Value::Number(8.0)]));
        let mut descriptor = ObjectData::default();
        descriptor.set("enumerable", Value::Boolean(false));
        let descriptor = Value::Object(Rc::new(RefCell::new(descriptor)));
        let realm = Realm::new();
        let mut host = TestHost { calls: Vec::new() };
        let mut vm = Vm::new(
            no_program(),
            Vec::new(),
            &mut host,
            ExecutionLimits::default(),
            &realm,
        );
        vm.dispatch_native(
            Native::ObjectDefineProperty,
            &Value::Undefined,
            &[target.clone(), Value::String("0".into()), descriptor],
            0,
        )
        .expect("define array index descriptor");

        assert_eq!(
            own_property_keys(&target),
            vec![
                PropertyKey::String("0".into()),
                PropertyKey::String("1".into()),
                PropertyKey::String("length".into()),
            ]
        );
        assert_eq!(
            own_properties(&target)
                .into_iter()
                .map(|(key, _)| key)
                .collect::<Vec<_>>(),
            vec![PropertyKey::String("1".into())]
        );
        let descriptor =
            own_descriptor(&target, &PropertyKey::String("0".into())).expect("index descriptor");
        let Value::Object(descriptor) = descriptor else {
            panic!("descriptor is an object");
        };
        assert_eq!(
            descriptor.borrow().get("enumerable"),
            Some(Value::Boolean(false))
        );
        assert_eq!(
            descriptor.borrow().get("writable"),
            Some(Value::Boolean(true))
        );
        assert_eq!(
            descriptor.borrow().get("configurable"),
            Some(Value::Boolean(true))
        );

        // A partial enumerable update retains the other property attributes.
        let mut partial = ObjectData::default();
        partial.set("enumerable", Value::Boolean(true));
        vm.dispatch_native(
            Native::ObjectDefineProperty,
            &Value::Undefined,
            &[
                target.clone(),
                Value::String("0".into()),
                Value::Object(Rc::new(RefCell::new(partial))),
            ],
            0,
        )
        .expect("partially update array index descriptor");
        assert_eq!(
            own_descriptor(&target, &PropertyKey::String("0".into())).and_then(|descriptor| {
                match descriptor {
                    Value::Object(descriptor) => descriptor.borrow().get("writable"),
                    _ => None,
                }
            }),
            Some(Value::Boolean(true))
        );
        assert!(delete_property(&target, &Value::String("0".into())));
    }

    #[test]
    fn partial_define_property_preserves_an_existing_accessor() {
        let mut source = ObjectData::default();
        source.define_accessor(
            "value",
            Some(Value::Native(Native::ArrayValues)),
            Some(Value::Native(Native::ArrayPush)),
        );
        let target = Value::Object(Rc::new(RefCell::new(source)));
        let mut descriptor = ObjectData::default();
        descriptor.set("enumerable", Value::Boolean(false));
        let descriptor = Value::Object(Rc::new(RefCell::new(descriptor)));
        let realm = Realm::new();
        let mut host = TestHost { calls: Vec::new() };
        let mut vm = Vm::new(
            no_program(),
            Vec::new(),
            &mut host,
            ExecutionLimits::default(),
            &realm,
        );
        vm.dispatch_native(
            Native::ObjectDefineProperty,
            &Value::Undefined,
            &[target.clone(), Value::String("value".into()), descriptor],
            0,
        )
        .expect("define property");
        assert!(matches!(
            target,
            Value::Object(object)
                if matches!(
                    object.borrow().get_own_property("value"),
                    Some(Property::Accessor { .. })
                )
        ));
    }

    #[test]
    fn computed_symbol_reads_invoke_accessors() {
        let symbol = Symbol::Unique(Rc::new(UniqueSymbol {
            serial: 100,
            description: None,
        }));
        let array = ArrayData::new(vec![Value::Number(1.0)]);
        array.properties.borrow_mut().define_accessor(
            symbol.clone(),
            Some(Value::Native(Native::ArrayValues)),
            None,
        );
        let value = Value::Array(array);
        let realm = Realm::new();
        let mut host = TestHost { calls: Vec::new() };
        let mut vm = Vm::new(
            no_program(),
            Vec::new(),
            &mut host,
            ExecutionLimits::default(),
            &realm,
        );
        let read = vm
            .read_computed(&value, &Value::Symbol(symbol), 0)
            .expect("symbol accessor read");
        assert!(matches!(read, Value::Object(_)), "getter must be invoked");
    }

    #[test]
    fn spreading_something_without_own_properties_contributes_nothing() {
        // `{ ...null }` is `{}`, not a throw -- the one place object spread is more forgiving
        // than array spread.
        assert!(own_properties(&Value::Null).is_empty());
        assert!(own_properties(&Value::Undefined).is_empty());
        assert!(own_properties(&Value::Number(1.0)).is_empty());
    }

    #[test]
    fn an_array_and_a_string_spread_as_their_indices() {
        let items = own_properties(&array(&[Value::Number(7.0), Value::Number(8.0)]));
        assert_eq!(items[0].0, "0");
        assert_eq!(items[1].0, "1");
        let characters = own_properties(&Value::String("hi".to_owned()));
        assert_eq!(characters.len(), 2);
        assert_eq!(characters[1].0, "1");
    }

    #[test]
    fn removing_a_property_keeps_the_order_of_the_rest() {
        // Insertion order is observable, so a removal must not be implemented as a swap.
        let mut data = ObjectData::default();
        for key in ["a", "b", "c"] {
            data.set(key, Value::Undefined);
        }
        data.remove("b");
        assert_eq!(data.keys().collect::<Vec<_>>(), vec!["a", "c"]);
    }

    #[test]
    fn an_index_key_enumerates_before_every_string_key() {
        // The order own keys enumerate in is observable through `Object.keys`, `JSON.stringify`,
        // spread and `for-in`, and it is not insertion order: indices come first, ascending.
        let mut data = ObjectData::default();
        for key in ["b", "10", "a", "2", "0"] {
            data.set(key, Value::Undefined);
        }
        assert_eq!(
            data.keys().collect::<Vec<_>>(),
            vec!["0", "2", "10", "b", "a"]
        );
    }

    #[test]
    fn only_a_canonical_index_counts_as_one() {
        // `"01"` and `"1.0"` name the same property as themselves and not as 1, so they are
        // ordinary string keys and enumerate in insertion order.
        assert_eq!(index_key("0"), Some(0));
        assert_eq!(index_key("10"), Some(10));
        assert_eq!(index_key("01"), None);
        assert_eq!(index_key("1.0"), None);
        assert_eq!(index_key("-1"), None);
        assert_eq!(index_key(""), None);
        // 2^32 - 1 is the length, never an index.
        assert_eq!(index_key("4294967295"), None);
        assert_eq!(index_key("4294967294"), Some(4_294_967_294));
    }

    #[test]
    fn reassigning_a_key_does_not_move_it() {
        let mut data = ObjectData::default();
        data.set("b", Value::Number(1.0));
        data.set("a", Value::Number(2.0));
        data.set("b", Value::Number(3.0));
        assert_eq!(data.keys().collect::<Vec<_>>(), vec!["b", "a"]);
    }

    #[test]
    fn a_negative_slice_bound_counts_from_the_end() {
        // The reason this is not a cast: `-2` means two from the end, not clamp-to-zero.
        assert_eq!(relative_index(Some(&Value::Number(-2.0)), 5, 0), 3);
        assert_eq!(relative_index(Some(&Value::Number(-99.0)), 5, 0), 0);
        assert_eq!(relative_index(Some(&Value::Number(2.0)), 5, 0), 2);
        assert_eq!(relative_index(Some(&Value::Number(99.0)), 5, 0), 5);
        // An absent or explicitly undefined bound takes the default, not zero.
        assert_eq!(relative_index(None, 5, 5), 5);
        assert_eq!(relative_index(Some(&Value::Undefined), 5, 5), 5);
    }

    fn parse_json(text: &str) -> Result<Value, ExecutionError> {
        let mut reader = JsonReader { rest: text };
        let value = reader.value()?;
        reader.skip_whitespace();
        if reader.rest.is_empty() {
            Ok(value)
        } else {
            Err(ExecutionError::InvalidJson)
        }
    }

    #[test]
    fn json_parses_each_kind_of_value() {
        assert!(matches!(parse_json("null"), Ok(Value::Null)));
        assert!(matches!(parse_json("true"), Ok(Value::Boolean(true))));
        assert!(matches!(parse_json(" -1.5e2 "), Ok(Value::Number(n)) if n == -150.0));
        assert!(matches!(parse_json(r#""s""#), Ok(Value::String(s)) if s == "s"));
        assert!(matches!(parse_json("[]"), Ok(Value::Array(_))));
        assert!(matches!(parse_json("{}"), Ok(Value::Object(_))));
    }

    #[test]
    fn json_parses_escapes_including_surrogates() {
        let Ok(Value::String(text)) = parse_json(r#""a\"b\\c\u00e9\n""#) else {
            panic!("expected a string");
        };
        assert_eq!(text, "a\"b\\cé\n");
        // A lone surrogate has no scalar value; replacing it beats failing the whole parse.
        let Ok(Value::String(lone)) = parse_json(r#""\ud800""#) else {
            panic!("expected a string");
        };
        assert_eq!(lone, "\u{fffd}");
    }

    #[test]
    fn json_rejects_what_is_not_json() {
        // Every one of these is accepted by some more permissive parser, which is why they are
        // listed: `f64::from_str` alone would take three of them.
        for text in [
            "",
            "{",
            "[1,]",
            "{\"a\":}",
            "'s'",
            "+1",
            ".5",
            "NaN",
            "inf",
            "01,",
            "nul",
            "{\"a\" 1}",
            "\"unterminated",
            "[1] extra",
        ] {
            assert!(
                parse_json(text).is_err(),
                "{text:?} should not parse as JSON"
            );
        }
    }

    #[test]
    fn a_parsed_object_keeps_the_last_value_for_a_duplicate_key() {
        let Ok(Value::Object(object)) = parse_json(r#"{"a":1,"a":2}"#) else {
            panic!("expected an object");
        };
        assert_eq!(object.borrow().keys().count(), 1);
        assert!(matches!(object.borrow().get_own("a"), Some(Value::Number(n)) if *n == 2.0));
    }

    #[test]
    fn a_console_shows_contents_where_string_shows_a_tag() {
        // The whole point of `inspect`: `String({a: 1})` is "[object Object]" and always will be.
        let value = object(&[
            ("a", Value::Number(1.0)),
            ("b", Value::String("x".to_owned())),
            ("c", Value::Null),
        ]);
        assert_eq!(inspect(&value), "{ a: 1, b: 'x', c: null }");
        assert_eq!(value.to_display_string(), "[object Object]");
    }

    #[test]
    fn a_top_level_string_is_bare_and_a_nested_one_is_quoted() {
        // `console.log("hi")` prints hi; `{ a: "hi" }` has to show that `a` holds a string.
        assert_eq!(inspect(&Value::String("hi".to_owned())), "hi");
        assert_eq!(
            inspect(&array(&[Value::String("hi".to_owned())])),
            "[ 'hi' ]"
        );
    }

    #[test]
    fn the_quote_character_is_whichever_the_string_lacks() {
        assert_eq!(quote_for_console("plain"), "'plain'");
        assert_eq!(quote_for_console("it's"), "\"it's\"");
        // Both quotes present but no backtick: node reaches for the backtick rather than escape.
        assert_eq!(quote_for_console("both ' and \""), "`both ' and \"`");
        // All three present, so something must be escaped and the preferred quote wins.
        assert_eq!(
            quote_for_console("all ' \" ` three"),
            "'all \\' \" ` three'"
        );
        assert_eq!(quote_for_console("a\tb\nc"), "'a\\tb\\nc'");
    }

    #[test]
    fn a_key_is_bare_only_when_it_is_a_plain_identifier() {
        // node's rule excludes `$` and anything starting with a digit, however odd that looks.
        assert_eq!(inspect_key("valid_1"), "valid_1");
        assert_eq!(inspect_key("_x"), "_x");
        assert_eq!(inspect_key("has $"), "'has $'");
        assert_eq!(inspect_key("with$"), "'with$'");
        assert_eq!(inspect_key("3"), "'3'");
        assert_eq!(inspect_key(""), "''");
    }

    #[test]
    fn empty_containers_and_nesting_read_the_way_node_writes_them() {
        assert_eq!(inspect(&object(&[])), "{}");
        assert_eq!(inspect(&array(&[])), "[]");
        assert_eq!(
            inspect(&array(&[array(&[Value::Number(1.0)])])),
            "[ [ 1 ] ]"
        );
    }

    #[test]
    fn a_cycle_prints_once_rather_than_forever() {
        let items = ArrayData::new(Vec::new());
        items.borrow_mut().push(Value::Array(Rc::clone(&items)));
        assert_eq!(inspect(&Value::Array(items)), "[ [Circular *1] ]");
    }

    #[test]
    fn nesting_past_the_depth_limit_becomes_a_placeholder() {
        // node stops descending at depth 2 and prints the kind instead.
        let deep = array(&[array(&[array(&[array(&[Value::Number(1.0)])])])]);
        assert_eq!(inspect(&deep), "[ [ [ [Array] ] ] ]");
    }

    #[test]
    fn a_symbol_inspection_shows_its_description() {
        let symbol = Value::Symbol(Symbol::WellKnown(WellKnown::Iterator));
        assert_eq!(inspect(&symbol), "Symbol(Symbol.iterator)");
    }

    #[test]
    fn bitwise_operands_truncate_to_thirty_two_bits() {
        // Not a cast: the value wraps modulo 2^32 rather than saturating, which is why
        // `1 << 31` is negative and `2 ** 32 | 0` is zero.
        assert_eq!(to_int32(1.0), 1);
        assert_eq!(to_int32(2147483648.0), i32::MIN);
        assert_eq!(to_int32(4294967296.0), 0);
        assert_eq!(to_int32(-1.5), -1);
        assert_eq!(to_int32(f64::NAN), 0);
        assert_eq!(to_int32(f64::INFINITY), 0);
        assert_eq!(to_uint32(-1.0), u32::MAX);
    }

    #[test]
    fn loose_equality_is_not_strict_equality_with_a_conversion() {
        // The three rules that are not conversions at all.
        assert!(loose_equal(&Value::Null, &Value::Undefined));
        assert!(!loose_equal(&Value::Null, &Value::Number(0.0)));
        assert!(!loose_equal(
            &Value::Number(f64::NAN),
            &Value::Number(f64::NAN)
        ));
        // A boolean becomes a number first, so `[] == false` goes by way of "" and 0.
        assert!(loose_equal(&array(&[]), &Value::Boolean(false)));
        assert!(loose_equal(
            &Value::Number(1.0),
            &Value::String("1".to_owned())
        ));
        assert!(loose_equal(
            &array(&[Value::Number(1.0)]),
            &Value::Number(1.0)
        ));
        // Two objects compare by identity even when their contents match.
        assert!(!loose_equal(&object(&[]), &object(&[])));
        let shared = object(&[]);
        assert!(loose_equal(&shared, &shared.clone()));
    }

    #[test]
    fn math_round_goes_toward_positive_infinity() {
        // Not `f64::round`, which rounds a half away from zero.
        let round = |value: f64| {
            let Ok(Value::Number(result)) = call_native(
                Native::MathRound,
                &Value::Undefined,
                &[Value::Number(value)],
            ) else {
                panic!("Math.round returns a number");
            };
            result
        };
        assert_eq!(round(1.5), 2.0);
        assert_eq!(round(-1.5), -1.0);
        assert_eq!(round(2.5), 3.0);
        // A negative input rounding to zero gives negative zero, which the floor loses.
        assert!(round(-0.5).is_sign_negative());
        assert_eq!(round(-0.5), 0.0);
    }

    #[test]
    fn parse_int_reads_as_far_as_it_can() {
        // The whole difference from `Number(text)`, which is NaN for every one of these.
        let parsed = |text: &str, radix: Option<f64>| {
            let mut arguments = vec![Value::String(text.to_owned())];
            if let Some(radix) = radix {
                arguments.push(Value::Number(radix));
            }
            let Value::Number(result) = parse_int(&arguments) else {
                panic!("parseInt returns a number");
            };
            result
        };
        assert_eq!(parsed("12px", None), 12.0);
        assert_eq!(parsed("  -42  ", None), -42.0);
        assert_eq!(parsed("0x1f", None), 31.0);
        assert_eq!(parsed("ff", Some(16.0)), 255.0);
        assert!(parsed("px", None).is_nan());
        // A radix outside 2..=36 has no meaning, and neither does an empty prefix.
        assert!(parsed("12", Some(1.0)).is_nan());
        assert!(parsed("", None).is_nan());
    }

    #[test]
    fn a_number_in_another_base_expands_its_fraction() {
        // There is no decimal spelling of 0.1 in base 3 to convert through, so the fraction is
        // produced by repeated multiplication instead.
        assert_eq!(number_in_radix(255.0, 16), "ff");
        assert_eq!(number_in_radix(255.0, 2), "11111111");
        assert_eq!(number_in_radix(0.5, 2), "0.1");
        assert_eq!(number_in_radix(-10.25, 4), "-22.1");
        assert_eq!(number_in_radix(0.0, 8), "0");
    }

    /// Run a program and return what it wrote to stdout, event loop included.
    fn stdout_of(program: &Program) -> String {
        struct Recorder {
            lines: Vec<String>,
        }
        impl HostOps for Recorder {
            fn call(
                &mut self,
                _operation: HostOp,
                arguments: &[HostValue],
            ) -> Result<HostValue, HostError> {
                self.lines.push(
                    arguments
                        .iter()
                        .map(HostValue::to_console_string)
                        .collect::<Vec<_>>()
                        .join(" "),
                );
                Ok(HostValue::Undefined)
            }
        }
        let mut host = Recorder { lines: Vec::new() };
        execute(
            Rc::new(program.clone()),
            &mut host,
            ExecutionLimits::default(),
        )
        .expect("runs");
        host.lines.join(",")
    }

    /// `setTimeout(() => console.log(tag), delay)` for each pair, in the order given.
    fn timer_program(schedule: &[(&str, f64)]) -> Program {
        let mut constants = Vec::new();
        let mut code = Vec::new();
        let mut functions = Vec::new();
        for (tag, delay) in schedule {
            let tag_index = constants.len() as u32;
            constants.push(Constant::String((*tag).to_owned()));
            let delay_index = constants.len() as u32;
            constants.push(number(*delay));

            code.push(Instruction::LoadBuiltin(kiln_ir::Builtin::SetTimeout));
            code.push(Instruction::MakeFunction(functions.len() as u32));
            code.push(Instruction::Const(delay_index));
            code.push(Instruction::Call { argc: 2 });
            code.push(Instruction::Pop);
            functions.push(kiln_ir::FunctionProto {
                asynchronous: false,
                name: None,
                arrow: false,
                generator: false,
                async_generator: false,
                parameter_count: 0,
                local_count: 0,
                code: vec![
                    Instruction::Const(tag_index),
                    Instruction::HostCall {
                        op: HostOp::WriteStdout,
                        argc: 1,
                    },
                    Instruction::Return,
                ],
                upvalues: Vec::new(),
            });
        }
        code.push(Instruction::Return);
        Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants,
            code,
            local_count: 0,
            functions,
        }
    }

    #[test]
    fn a_timers_delay_costs_nothing_to_wait_for() {
        // The property the differential harness cannot check, because node would sleep for it:
        // an hour-long delay is as cheap as a zero-length one, and the order is still by time.
        let program = timer_program(&[("hour", 3_600_000.0), ("second", 1_000.0), ("now", 0.0)]);
        assert_eq!(stdout_of(&program), "now,second,hour");
    }

    #[test]
    fn timers_due_together_run_in_the_order_they_were_scheduled() {
        // A due time is not a total order, so without a sequence number this depends on however
        // the queue happens to be arranged.
        let program = timer_program(&[("first", 0.0), ("second", 0.0), ("third", 0.0)]);
        assert_eq!(stdout_of(&program), "first,second,third");
    }

    #[test]
    fn a_program_is_not_finished_while_it_still_has_work_scheduled() {
        // Returning at the end of the module body would make every timer unobservable.
        let program = timer_program(&[("scheduled", 5.0)]);
        assert_eq!(stdout_of(&program), "scheduled");
    }

    #[test]
    fn an_async_arrow_captures_its_defining_receiver() {
        let maker = kiln_ir::FunctionProto {
            asynchronous: false,
            name: None,
            arrow: false,
            generator: false,
            async_generator: false,
            parameter_count: 0,
            local_count: 0,
            code: vec![Instruction::MakeFunction(1), Instruction::Return],
            upvalues: Vec::new(),
        };
        let arrow = kiln_ir::FunctionProto {
            asynchronous: true,
            name: None,
            arrow: true,
            generator: true,
            async_generator: false,
            parameter_count: 0,
            local_count: 0,
            code: vec![
                Instruction::LoadThis,
                Instruction::GetProperty(1),
                Instruction::Return,
            ],
            upvalues: Vec::new(),
        };
        let report = kiln_ir::FunctionProto {
            asynchronous: false,
            name: None,
            arrow: false,
            generator: false,
            async_generator: false,
            parameter_count: 1,
            local_count: 1,
            code: vec![
                Instruction::LoadLocal(0),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Return,
            ],
            upvalues: Vec::new(),
        };
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![
                Constant::String("make".into()),
                Constant::String("tag".into()),
                Constant::String("owner".into()),
                Constant::String("wrong".into()),
                Constant::String("call".into()),
                Constant::String("then".into()),
            ],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::StoreLocal(0),
                Instruction::Const(1),
                Instruction::Const(2),
                Instruction::NewObject { count: 1 },
                Instruction::StoreLocal(1),
                Instruction::LoadLocal(1),
                Instruction::LoadLocal(0),
                Instruction::SetProperty(0),
                Instruction::Pop,
                Instruction::LoadLocal(1),
                Instruction::CallMethod { name: 0, argc: 0 },
                Instruction::StoreLocal(2),
                Instruction::Const(1),
                Instruction::Const(3),
                Instruction::NewObject { count: 1 },
                Instruction::StoreLocal(3),
                Instruction::LoadLocal(2),
                Instruction::LoadLocal(3),
                Instruction::CallMethod { name: 4, argc: 1 },
                Instruction::MakeFunction(2),
                Instruction::CallMethod { name: 5, argc: 1 },
                Instruction::Pop,
                Instruction::Return,
            ],
            local_count: 4,
            functions: vec![maker, arrow, report],
        };

        assert_eq!(stdout_of(&program), "owner");
    }
}
