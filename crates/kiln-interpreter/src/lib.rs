//! A small, deterministic stack interpreter for Kiln's parser-independent IR.
//!
//! The interpreter deliberately knows nothing about the frontend.  It consumes only the owned
//! bytecode in `kiln-ir` and reaches the outside world through the versioned `HostOps` trait.  It
//! is intentionally a scalar runtime for the first execution slice; object allocation,
//! exceptions, and async scheduling belong above this VM.

use std::fmt;

use std::cell::RefCell;
use std::rc::Rc;

use kiln_host_abi::{HostOps, HostValue};
use kiln_ir::{BinaryOp, Constant, Instruction, ModuleProgram, Program, UnaryOp, Upvalue};

/// The built-ins the runtime provides natively.
///
/// Kept minimal on purpose. design.md §5 wants built-ins written in JavaScript, and everything
/// here is a placeholder for one -- but nothing can bootstrap a JS built-in until there are
/// prototypes and a loader, so the handful the conformance corpus needs first live here.
/// The well-known symbols, which are the only symbols that exist so far.
///
/// Symbol-keyed properties are stored under a reserved string key (`@@iterator`), the same
/// convention engines use internally. It is not airtight -- a program could write that string as
/// a literal key -- but a separate key space costs a second lookup path everywhere for a
/// collision nobody writes by accident, and the corpus is what would catch it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WellKnown {
    Iterator,
}

impl WellKnown {
    pub fn property_key(self) -> &'static str {
        match self {
            Self::Iterator => "@@iterator",
        }
    }

    /// What `String(symbol)` shows, which spells the well-known ones out.
    fn description(self) -> &'static str {
        match self {
            Self::Iterator => "Symbol.iterator",
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

/// Any string key beginning with this is the VM's own: a symbol-keyed property or an internal
/// slot. `Object.keys` and `JSON.stringify` enumerate own *string* keys, so both skip them.
const RESERVED_KEY_PREFIX: &str = "@@";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Native {
    Error,
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
    /// `Math.random` is deliberately absent: a nondeterministic builtin needs a seeding decision
    /// this compiler has not made, and an unseeded one would make every artifact hash a lie.
    Math,
    MathAbs,
    MathFloor,
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
    MapSet,
    MapGet,
    MapHas,
    MapDelete,
    CollectionValues,
    CollectionIteratorNext,
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
    fn property(self, name: &str) -> Option<Value> {
        match (self, name) {
            (Self::Error, "captureStackTrace") => Some(Value::Native(Self::ErrorCaptureStackTrace)),
            (Self::Error, "name") => Some(Value::String("Error".to_owned())),
            (Self::ArrayConstructor, "isArray") => Some(Value::Native(Self::ArrayIsArray)),
            (Self::ArrayConstructor, "from") => Some(Value::Native(Self::ArrayFrom)),
            (Self::ObjectConstructor, "keys") => Some(Value::Native(Self::ObjectKeys)),
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
            (Self::Json, "parse") => Some(Value::Native(Self::JsonParse)),
            (Self::Json, "stringify") => Some(Value::Native(Self::JsonStringify)),
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

#[derive(Debug, Default)]
pub struct ObjectData {
    properties: Vec<(String, Value)>,
    /// The `[[Prototype]]` link. Reads fall through it; writes never do -- assigning a property
    /// that exists only on the prototype creates an own property rather than mutating the shared
    /// one, which is what keeps two instances of a class independent.
    prototype: Option<Value>,
}

impl ObjectData {
    /// An own property, ignoring the prototype chain.
    pub fn get_own(&self, key: &str) -> Option<&Value> {
        self.properties
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// A property, following the prototype chain.
    ///
    /// Returns an owned value rather than a reference because the answer may live in another
    /// object behind an `Rc<RefCell<_>>`, whose borrow cannot outlive this call.
    pub fn get(&self, key: &str) -> Option<Value> {
        if let Some(own) = self.get_own(key) {
            return Some(own.clone());
        }
        match &self.prototype {
            Some(Value::Object(parent)) => parent.borrow().get(key),
            // A class's statics are inherited through the constructor, and a constructor is a
            // function whose own properties live in its own store. Without this arm the chain
            // would stop at the first function in it.
            Some(Value::Function(parent)) => parent.properties.borrow().get(key),
            _ => None,
        }
    }

    pub fn prototype(&self) -> Option<&Value> {
        self.prototype.as_ref()
    }

    pub fn set_prototype(&mut self, prototype: Value) {
        self.prototype = Some(prototype);
    }

    /// Insert or overwrite, keeping the store in enumeration order.
    ///
    /// Re-assigning a property does not move it to the end in JavaScript, so it must not here.
    /// A *new* key is placed where enumeration would put it rather than simply appended: an
    /// array index comes before every string key, in ascending numeric order, however it was
    /// written. Ordering on insert rather than on read means `Object.keys`, `JSON.stringify`,
    /// spread and `for-in` are all correct without any of them knowing the rule.
    pub fn set(&mut self, key: &str, value: Value) {
        if let Some((_, existing)) = self.properties.iter_mut().find(|(name, _)| name == key) {
            *existing = value;
            return;
        }
        let Some(index) = index_key(key) else {
            self.properties.push((key.to_owned(), value));
            return;
        };
        // The indices already form a sorted prefix, so this is where the new one joins it.
        let position = self
            .properties
            .iter()
            .position(|(name, _)| index_key(name).is_none_or(|other| other > index))
            .unwrap_or(self.properties.len());
        self.properties.insert(position, (key.to_owned(), value));
    }

    /// Remove an own property, keeping the order of the rest.
    pub fn remove(&mut self, key: &str) {
        self.properties.retain(|(name, _)| name != key);
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.properties.iter().map(|(name, _)| name.as_str())
    }

    /// Own properties in insertion order, which is the order JavaScript enumerates them in.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.properties
            .iter()
            .map(|(name, value)| (name.as_str(), value))
    }
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
}

impl PartialEq for GeneratorState {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// A callable value: which function body to run, and the bindings it closed over.
#[derive(Debug)]
pub struct Closure {
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
    /// An array. Separate from `Object` rather than an object with numeric keys: `Array.isArray`
    /// has to be able to tell them apart, `length` is not an ordinary property, and a dense
    /// `Vec` is the representation that makes indexing the common case it is.
    Array(Rc<RefCell<Vec<Value>>>),
    /// A built-in that takes a callback, invoked by the VM rather than by `call_native`.
    Callback(CallbackMethod),
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
    /// Distinct from `to_display_string`: a symbol keys a property under its reserved slot, not
    /// under its human-readable description, and conflating the two would make
    /// `obj[Symbol.iterator]` and `obj["Symbol(Symbol.iterator)"]` the same property.
    pub fn property_key(&self) -> String {
        match self {
            Self::Symbol(symbol) => symbol.property_key(),
            other => other.to_display_string(),
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
            Self::Native(_) | Self::Callback(_) => "function",
            Self::Array(_) | Self::Generator(_) => "object",
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
            Self::Native(_) | Self::Callback(_) => "function",
            Self::Array(_) => "array",
            Self::Generator(_) => "generator",
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
            Self::Object(_) | Self::Native(_) | Self::Callback(_) | Self::Generator(_) => f64::NAN,
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
            | Self::Symbol(_) => true,
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
            Self::Native(native) => format!("[Function {native:?}]"),
            Self::Callback(method) => format!("[Function {method:?}]"),
            Self::Generator(_) => "[object Generator]".to_owned(),
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
            max_call_depth: 512,
        }
    }
}

/// The successful result of one VM invocation.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionResult {
    pub value: Value,
    pub steps: u64,
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
    },
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
            Self::RadixOutOfRange => {
                write!(
                    formatter,
                    "argument is out of the range this method accepts"
                )
            }
            Self::EmptyReduce => write!(formatter, "reduce of empty array with no initial value"),
            Self::InvalidJson => write!(formatter, "unexpected token in JSON"),
            Self::CyclicJson => write!(formatter, "converting circular structure to JSON"),
            Self::GeneratorReturned => write!(formatter, "generator returned"),
            Self::Uncaught { message, .. } => write!(formatter, "uncaught {message}"),
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

impl std::error::Error for ExecutionError {}

/// Execute one IR program against a host implementation.
pub fn execute<H: HostOps + ?Sized>(
    program: &Program,
    host: &mut H,
    limits: ExecutionLimits,
) -> Result<ExecutionResult, ExecutionError> {
    let mut vm = Vm {
        program,
        host,
        stack: Vec::new(),
        locals: vec![Value::Undefined; program.local_count as usize],
        function: None,
        suspended: false,
        handlers: Vec::new(),
        arguments: Vec::new(),
        error_prototype: None,
        this_value: Value::Undefined,
        frames: Vec::new(),
        instruction: 0,
        function_names: program
            .functions
            .iter()
            .map(|proto| proto.name.as_deref().map(Rc::from))
            .collect(),
        steps: 0,
        symbol_serial: 0,
        limits,
    };
    vm.run()
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
        *local = value.clone();
    }

    let mut vm = Vm {
        program: &module.program,
        host,
        stack: Vec::new(),
        locals,
        function: None,
        suspended: false,
        handlers: Vec::new(),
        arguments: Vec::new(),
        error_prototype: None,
        this_value: Value::Undefined,
        frames: Vec::new(),
        instruction: 0,
        function_names: module
            .program
            .functions
            .iter()
            .map(|proto| proto.name.as_deref().map(Rc::from))
            .collect(),
        steps: 0,
        symbol_serial: 0,
        limits,
    };
    let execution = vm.run()?;
    let mut exports = Vec::with_capacity(module.exports.len());
    for binding in &module.exports {
        let Some(value) = vm.locals.get(binding.local_slot as usize) else {
            return Err(ExecutionError::LocalOutOfBounds {
                index: binding.local_slot as usize,
            });
        };
        exports.push(ModuleExportValue {
            name: binding.exported.clone(),
            value: value.clone(),
        });
    }
    Ok(ModuleExecutionResult { execution, exports })
}

/// A `catch` or `finally` waiting for an exception.
///
/// The frame depth and stack height are recorded so unwinding can restore the machine to exactly
/// the state the handler expects, however many frames the throw crossed.
#[derive(Debug)]
struct Handler {
    target: usize,
    frame_depth: usize,
    stack_height: usize,
}

/// A suspended caller, restored when the callee returns.
///
/// The module body is not represented here: it is the bottom of the call stack rather than a
/// frame in it, which is why an empty `frames` means `Return` ends execution.
struct Frame {
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

struct Vm<'program, 'host, H: HostOps + ?Sized> {
    program: &'program Program,
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
    /// `Error.prototype` for this execution, created on first use.
    ///
    /// One object per run, because `e instanceof Error` compares it by identity: two error
    /// prototypes would make errors from the same program fail to recognise each other. This is
    /// the smallest thing a realm is, and it is where a real realm object will start.
    error_prototype: Option<Value>,
    /// `this` in the running frame. At module scope it is `undefined`, because a module body is
    /// always strict and strict code does not get the global object.
    this_value: Value,
    frames: Vec<Frame>,
    instruction: usize,
    /// One shared name per proto, so making a closure copies a pointer rather than a string.
    function_names: Vec<Option<Rc<str>>>,
    steps: u64,
    /// Serial for the next `Symbol(...)`. Monotonic per execution, which is all the identity a
    /// symbol needs: it never leaves the run that made it.
    symbol_serial: u64,
    limits: ExecutionLimits,
}

impl<'program, H: HostOps + ?Sized> Vm<'program, '_, H> {
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
        loop {
            if self.steps >= self.limits.max_steps {
                return Err(ExecutionError::StepLimitExceeded {
                    steps: self.steps,
                    limit: self.limits.max_steps,
                });
            }
            let Some(opcode) = self.code().get(self.instruction) else {
                return Err(ExecutionError::InvalidInstructionPointer {
                    instruction: self.instruction,
                });
            };
            let current = self.instruction;
            self.steps += 1;
            self.instruction += 1;

            match opcode {
                Instruction::Const(index) => {
                    let Some(value) = self.program.constants.get(*index as usize) else {
                        return Err(ExecutionError::ConstantOutOfBounds {
                            index: *index as usize,
                        });
                    };
                    self.stack.push(value.clone().into());
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
                    self.stack.push(unary(*operator, value));
                }
                Instruction::Binary(BinaryOp::InstanceOf) => {
                    let right = self.pop(current)?;
                    let left = self.pop(current)?;
                    // Threaded rather than global: `Error.prototype` belongs to this run.
                    let prototype = self.error_prototype.clone();
                    self.stack.push(Value::Boolean(instance_of(
                        &left,
                        &right,
                        prototype.as_ref(),
                    )));
                }
                Instruction::Binary(operator) => {
                    let right = self.pop(current)?;
                    let left = self.pop(current)?;
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
                    let method = self.property(&receiver, &name);
                    let result = self.invoke(method, receiver, arguments, current)?;
                    if let Some(result) = result {
                        self.stack.push(result);
                    }
                }
                Instruction::CallComputed { argc } => {
                    let arguments = self.take_call_arguments(*argc, current)?;
                    let key = self.pop(current)?;
                    let receiver = self.pop(current)?;
                    let method = self.computed(&receiver, &key);
                    let result = self.invoke(method, receiver, arguments, current)?;
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
                    self.stack.push(Value::Array(Rc::new(RefCell::new(items))));
                }
                Instruction::IterableToArray => {
                    let source = self.pop(current)?;
                    // An array is already the right shape; anything else goes through the
                    // protocol, so `const [a, b] = someSet` binds what iterating it would yield.
                    let items = match &source {
                        Value::Array(items) => items.borrow().clone(),
                        other => self.drain_iterable(other, current)?,
                    };
                    self.stack.push(Value::Array(Rc::new(RefCell::new(items))));
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
                    match &source {
                        Value::Array(source) => {
                            let appended = source.borrow().clone();
                            items.borrow_mut().extend(appended);
                        }
                        other => {
                            let collected = self.drain_iterable(other, current)?;
                            items.borrow_mut().extend(collected);
                        }
                    }
                    self.stack.push(target);
                }
                Instruction::GetComputed => {
                    let key = self.pop(current)?;
                    let target = self.pop(current)?;
                    self.stack.push(self.computed(&target, &key));
                }
                Instruction::SetComputed => {
                    let value = self.pop(current)?;
                    let key = self.pop(current)?;
                    let target = self.pop(current)?;
                    self.set_computed(&target, &key, value.clone(), current)?;
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
                    });
                }
                Instruction::PopHandler => {
                    self.handlers.pop();
                }
                Instruction::Yield => {
                    // Returning straight out of the loop leaves the generator's registers in
                    // place; the caller snapshots them. A `Return` would have popped the frame.
                    let value = self.pop(current)?;
                    self.suspended = true;
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
                    let arguments = self.arguments.clone();
                    self.stack
                        .push(Value::Array(Rc::new(RefCell::new(arguments))));
                }
                Instruction::LoadThis => {
                    let this_value = self.this_value.clone();
                    self.stack.push(this_value);
                }
                Instruction::GetProperty(name)
                    if matches!(self.stack.last(), Some(Value::Native(Native::Error)))
                        && self.string_constant(*name, current)? == "prototype" =>
                {
                    // `Error.prototype` has to be the same object instances inherit from, or
                    // `extends Error` would link to something no error descends from.
                    self.stack.pop();
                    let prototype = self.error_prototype();
                    self.stack.push(prototype);
                }
                Instruction::LoadBuiltin(builtin) => {
                    self.stack.push(Value::Native(match builtin {
                        kiln_ir::Builtin::Error => Native::Error,
                        kiln_ir::Builtin::Symbol => Native::SymbolConstructor,
                        kiln_ir::Builtin::String => Native::StringConstructor,
                        kiln_ir::Builtin::Set => Native::SetConstructor,
                        kiln_ir::Builtin::Map => Native::MapConstructor,
                        // The realm's global object, holding exactly the built-ins that exist.
                        // A fresh object each time is wrong -- `globalThis === globalThis` should
                        // hold -- but nothing observes that yet, and pinning it needs the realm
                        // this is the first piece of.
                        kiln_ir::Builtin::GlobalThis => {
                            let mut globals = ObjectData::default();
                            globals.set("Error", Value::Native(Native::Error));
                            globals.set("Array", Value::Native(Native::ArrayConstructor));
                            globals.set("Object", Value::Native(Native::ObjectConstructor));
                            globals.set("JSON", Value::Native(Native::Json));
                            globals.set("Symbol", Value::Native(Native::SymbolConstructor));
                            globals.set("String", Value::Native(Native::StringConstructor));
                            globals.set("Math", Value::Native(Native::Math));
                            globals.set("Number", Value::Native(Native::NumberConstructor));
                            globals.set("Boolean", Value::Native(Native::BooleanConstructor));
                            globals.set("NaN", Value::Number(f64::NAN));
                            globals.set("Infinity", Value::Number(f64::INFINITY));
                            self.stack
                                .push(Value::Object(Rc::new(RefCell::new(globals))));
                            continue;
                        }
                        kiln_ir::Builtin::Array => Native::ArrayConstructor,
                        kiln_ir::Builtin::Object => Native::ObjectConstructor,
                        kiln_ir::Builtin::Json => Native::Json,
                        kiln_ir::Builtin::Math => Native::Math,
                        kiln_ir::Builtin::Number => Native::NumberConstructor,
                        kiln_ir::Builtin::Boolean => Native::BooleanConstructor,
                        kiln_ir::Builtin::ParseInt => Native::ParseInt,
                        kiln_ir::Builtin::ParseFloat => Native::ParseFloat,
                        kiln_ir::Builtin::IsNaN => Native::IsNaN,
                        kiln_ir::Builtin::IsFinite => Native::IsFinite,
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
                    match callee {
                        Value::Native(Native::Error) => {
                            let error = self.construct_error(&arguments);
                            self.stack.push(error);
                        }
                        Value::Native(kind @ (Native::SetConstructor | Native::MapConstructor)) => {
                            let is_map = matches!(kind, Native::MapConstructor);
                            let mut data = ObjectData::default();
                            data.set("@@entries", Value::Array(Rc::new(RefCell::new(Vec::new()))));
                            if is_map {
                                data.set("@@map", Value::Boolean(true));
                            }
                            data.set("size", Value::Number(0.0));
                            let collection = Value::Object(Rc::new(RefCell::new(data)));
                            // `new Set(iterable)` and `new Map(entries)` seed themselves through
                            // `add`/`set`, so deduplication and size accounting are the same code
                            // the explicit calls use rather than a second copy of it.
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
                        object.set(&pair[0].property_key(), pair[1].clone());
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
                    for (key, value) in own_properties(&source) {
                        object.borrow_mut().set(&key, value);
                    }
                    self.stack.push(target);
                }
                Instruction::DeleteProperty(name) => {
                    let name = self.string_constant(*name, current)?;
                    let target = self.pop(current)?;
                    if let Value::Object(object) = &target {
                        object.borrow_mut().remove(&name);
                    }
                    // `delete` reports whether the property is gone afterwards, and on anything
                    // without own properties it trivially is.
                    self.stack.push(Value::Boolean(true));
                }
                Instruction::GetProperty(name) => {
                    let name = self.string_constant(*name, current)?;
                    let target = self.pop(current)?;
                    self.stack.push(self.property(&target, &name));
                }
                Instruction::SetProperty(name) => {
                    let name = self.string_constant(*name, current)?;
                    let value = self.pop(current)?;
                    let target = self.pop(current)?;
                    // Assigning a property of a primitive is silently discarded in sloppy mode
                    // and a TypeError in strict mode. Modules are always strict, and there is
                    // nowhere to throw yet, so it is an execution error.
                    self.set_computed(&target, &Value::String(name), value.clone(), current)?;
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
                    self.stack.push(value);
                }
                Instruction::StoreCell(slot) => {
                    let value = self.pop(current)?;
                    let cell = self.cell(*slot as usize)?;
                    *cell.borrow_mut() = value;
                }
                Instruction::LoadUpvalue(index) => {
                    let value = self.upvalue(*index)?.borrow().clone();
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
                            self.call(closure, argc, current)?;
                        }
                        Value::Native(native) => {
                            let native = *native;
                            let arguments = self.take_arguments(argc, current)?;
                            let result = self.dispatch_native(
                                native,
                                &Value::Undefined,
                                &arguments,
                                current,
                            )?;
                            self.stack.push(result);
                        }
                        other => {
                            return Err(ExecutionError::NotCallable {
                                instruction: current,
                                value: other.type_name(),
                            });
                        }
                    }
                }
                Instruction::LoadGlobal(slot) => {
                    let value = self.global(*slot as usize)?;
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
                    self.function = frame.function;
                    self.this_value = frame.this_value;
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
        let Some(handler) = self.handlers.pop() else {
            // A `.return()` signal that reaches the top has run every `finally` between the
            // suspension point and here, which is the whole reason it was thrown.
            if matches!(thrown, Value::Native(Native::ReturnSignal)) {
                return Err(ExecutionError::GeneratorReturned);
            }
            return Err(ExecutionError::Uncaught {
                instruction,
                message: describe_thrown(&thrown),
            });
        };
        while self.frames.len() > handler.frame_depth {
            let frame = self.frames.pop().expect("checked by the loop condition");
            self.locals = frame.locals;
            self.function = frame.function;
            self.this_value = frame.this_value;
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
    ) -> Result<Value, ExecutionError> {
        let proto = &self.program.functions[closure.function as usize];
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
            this_value: Value::Undefined,
            arguments: received,
            started: false,
            done: false,
        }))))
    }

    /// Start or resume a generator, returning its next value and whether it finished.
    fn resume_generator(
        &mut self,
        generator: &Rc<RefCell<GeneratorState>>,
        sent: Value,
        instruction: usize,
    ) -> Result<(Value, bool), ExecutionError> {
        self.resume_generator_with(generator, sent, instruction, false)
    }

    /// Resume a generator, optionally by throwing the `.return()` signal into it.
    fn resume_generator_with(
        &mut self,
        generator: &Rc<RefCell<GeneratorState>>,
        sent: Value,
        instruction: usize,
        returning: bool,
    ) -> Result<(Value, bool), ExecutionError> {
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
                state.arguments.clone(),
                state.started,
            )
        };

        self.frames.push(Frame {
            function: self.function.take(),
            this_value: std::mem::replace(&mut self.this_value, this_value),
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
            });
        }
        if started && !returning {
            // The value the caller sent becomes the result of the `yield` that suspended. A
            // `.return()` does not resume the yield at all, so it pushes nothing.
            self.stack.push(sent.clone());
        }

        self.suspended = false;
        let outcome = if returning {
            // Throwing at the suspension point runs every `finally` between there and the top of
            // the generator, then arrives as `GeneratorReturned`.
            match self
                .unwind(Value::Native(Native::ReturnSignal), instruction)
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
        self.function = frame.function;
        self.this_value = frame.this_value;
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
        match native {
            Native::ArrayFrom => self.array_from(arguments, instruction),
            // Every call has to produce a symbol distinct from every other, so this needs the
            // counter rather than only its arguments.
            Native::SymbolConstructor => {
                self.symbol_serial += 1;
                Ok(Value::Symbol(Symbol::Unique(Rc::new(UniqueSymbol {
                    serial: self.symbol_serial,
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
            // Already the right shape, and copying beats walking an iterator to the same elements.
            Value::Array(items) => items.borrow().clone(),
            other
                if self
                    .property(other, WellKnown::Iterator.property_key())
                    .is_nullish() =>
            {
                let length = match self.property(other, "length") {
                    Value::Number(length) if length >= 1.0 => length as usize,
                    _ => 0,
                };
                (0..length)
                    .map(|index| self.computed(&source, &Value::Number(index as f64)))
                    .collect()
            }
            other => self.drain_iterable(other, instruction)?,
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
        Ok(Value::Array(Rc::new(RefCell::new(items))))
    }

    fn drain_iterable(
        &mut self,
        value: &Value,
        instruction: usize,
    ) -> Result<Vec<Value>, ExecutionError> {
        let method = self.property(value, WellKnown::Iterator.property_key());
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
            let result = self.call_value(
                &callback,
                this_value.clone(),
                vec![item.clone(), Value::Number(index as f64), receiver.clone()],
                instruction,
            )?;
            match method {
                // Dispatched before `array_callback` is reached; a generator method is not an
                // array walk.
                CallbackMethod::GeneratorNext | CallbackMethod::GeneratorReturn => {
                    unreachable!("generator methods are handled by `invoke`")
                }
                CallbackMethod::Reduce => {
                    unreachable!("`reduce` is handled by `array_reduce`")
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
            }
            index += 1;
        }

        Ok(match method {
            CallbackMethod::GeneratorNext | CallbackMethod::GeneratorReturn => {
                unreachable!("generator methods are handled by `invoke`")
            }
            CallbackMethod::Reduce => unreachable!("`reduce` is handled by `array_reduce`"),
            CallbackMethod::Map | CallbackMethod::Filter => {
                Value::Array(Rc::new(RefCell::new(collected)))
            }
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
    fn array_reduce(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Array(items) = receiver else {
            return Ok(Value::Undefined);
        };
        let callback = arguments.first().cloned().unwrap_or(Value::Undefined);
        let (mut accumulator, mut index) = match arguments.get(1) {
            Some(initial) => (initial.clone(), 0usize),
            None => {
                // Reducing an empty array with no initial value has no answer to give, which is
                // a TypeError in the specification and fatal here for want of a way to throw one.
                let Some(first) = items.borrow().first().cloned() else {
                    return Err(ExecutionError::EmptyReduce);
                };
                (first, 1usize)
            }
        };
        loop {
            // The length is re-read each step, so a callback that shortens the array stops the
            // walk rather than reading past the end -- the same rule `array_callback` follows.
            let Some(item) = items.borrow().get(index).cloned() else {
                return Ok(accumulator);
            };
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
            index += 1;
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
        match callee {
            Value::Native(native) => {
                self.dispatch_native(*native, &this_value, &arguments, instruction)
            }
            // A callback method reaches here when a built-in drives another built-in -- a spread
            // over a generator, for instance. `invoke` is the one place that knows how to run
            // each kind, so this defers to it rather than repeating the dispatch.
            Value::Callback(_) => Ok(self
                .invoke(callee.clone(), this_value, arguments, instruction)?
                .unwrap_or(Value::Undefined)),
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
    fn code(&self) -> &'program [Instruction] {
        match &self.function {
            None => &self.program.code,
            // Validated when the closure was created; a call never installs a bad index.
            Some(closure) => &self.program.functions[closure.function as usize].code,
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
                self.call_with_this(closure, argc as u16, current, this_value, None)?;
            }
            Value::Native(Native::Error) => {
                // A native parent leaves no frame, so the expression's value has to
                // be pushed here to match what a compiled parent's `Return` does.
                self.stack.push(Value::Undefined);
                // A native constructor builds its own object, so its own properties
                // are copied onto the instance. That is what `class E extends Error`
                // needs: message, name, stack and cause end up on `this`.
                let built = self.construct_error(&arguments);
                if let (Value::Object(instance), Value::Object(built)) = (&this_value, &built) {
                    let built = built.borrow();
                    let mut instance = instance.borrow_mut();
                    for key in built.keys().map(str::to_owned).collect::<Vec<_>>() {
                        if let Some(value) = built.get_own(&key) {
                            instance.set(&key, value.clone());
                        }
                    }
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

    /// `Error.prototype`, created on first use.
    fn error_prototype(&mut self) -> Value {
        if let Some(prototype) = &self.error_prototype {
            return prototype.clone();
        }
        let mut data = ObjectData::default();
        data.set("name", Value::String("Error".to_owned()));
        data.set("message", Value::String(String::new()));
        // Marks the chain for `String(e)`. A real `toString` would be invoked through the normal
        // method path; this is the placeholder that keeps the observable behaviour right until
        // built-ins can be written in JavaScript (design.md §5).
        data.set("toString", Value::Native(Native::ErrorToString));
        let prototype = Value::Object(Rc::new(RefCell::new(data)));
        self.error_prototype = Some(prototype.clone());
        prototype
    }

    /// Build an `Error` instance whose prototype is this run's `Error.prototype`.
    fn construct_error(&mut self, arguments: &[Value]) -> Value {
        let prototype = self.error_prototype();
        let error = build_error(arguments);
        if let Value::Object(object) = &error {
            object.borrow_mut().set_prototype(prototype);
        }
        error
    }

    /// The object a `new` call starts from: an empty object whose prototype is the function's.
    ///
    /// The prototype object is created on first use rather than with every closure, so an
    /// ordinary function never used as a constructor never allocates one.
    fn instantiate(&mut self, closure: &Rc<Closure>) -> Value {
        let prototype = closure
            .prototype
            .borrow()
            .clone()
            .unwrap_or_else(|| Value::Object(Rc::new(RefCell::new(ObjectData::default()))));
        *closure.prototype.borrow_mut() = Some(prototype.clone());

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
            name: self.function_names[function as usize].clone(),
            upvalues,
            // An arrow takes `this` from where it is written; an ordinary function takes it from
            // how it is called, so it captures nothing here.
            captured_this: if proto.arrow {
                Some(self.this_value.clone())
            } else {
                None
            },
            captured_arguments: if proto.arrow {
                Some(self.arguments.clone())
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
    fn property(&self, target: &Value, name: &str) -> Value {
        match target {
            // A Set or Map instance is an object whose reserved state marks it. Its methods come
            // from one table shared with `in`, so the two cannot disagree -- and this arm has to
            // precede the ordinary object case, which would otherwise answer `undefined` first.
            Value::Object(object)
                if object.borrow().get_own("@@entries").is_some() && name != "size" =>
            {
                let is_map = matches!(
                    object.borrow().get_own("@@map"),
                    Some(&Value::Boolean(true))
                );
                match collection_property(is_map, name) {
                    Value::Undefined => object.borrow().get(name).unwrap_or(Value::Undefined),
                    method => method,
                }
            }
            Value::Object(object) => object.borrow().get(name).unwrap_or(Value::Undefined),
            Value::Native(native) => native.property(name).unwrap_or(Value::Undefined),
            // `f.prototype` is the object `new f()` gives instances. Reading it creates it, so
            // that `f.prototype.m = ...` before any `new` reaches the same object the instances
            // will see.
            Value::Function(closure) if name != "prototype" => closure
                .properties
                .borrow()
                .get(name)
                .unwrap_or(Value::Undefined),
            Value::Function(closure) if name == "prototype" => {
                let mut slot = closure.prototype.borrow_mut();
                slot.get_or_insert_with(|| {
                    Value::Object(Rc::new(RefCell::new(ObjectData::default())))
                })
                .clone()
            }
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
            Value::Array(_) => array_property(name),
            // A generator object is its own iterator, which is what lets one be used directly in
            // `for-of` and in a spread.
            Value::Generator(_) => match name {
                "next" => Value::Callback(CallbackMethod::GeneratorNext),
                "return" => Value::Callback(CallbackMethod::GeneratorReturn),
                key if key == WellKnown::Iterator.property_key() => {
                    Value::Native(Native::IteratorSelf)
                }
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
            Value::Native(native) => self
                .dispatch_native(native, &receiver, &arguments, instruction)
                .map(Some),
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
                        self.resume_generator_with(&generator, sent, instruction, true)?
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
            // `reduce` threads an accumulator, so it is not the same walk as the others.
            Value::Callback(CallbackMethod::Reduce) => self
                .array_reduce(&receiver, &arguments, instruction)
                .map(Some),
            Value::Callback(method) => Ok(Some(self.array_callback(
                method,
                &receiver,
                &arguments,
                instruction,
            )?)),
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
    fn computed(&self, target: &Value, key: &Value) -> Value {
        if let Value::Array(items) = target
            && let Some(index) = array_index(key)
        {
            return items
                .borrow()
                .get(index)
                .cloned()
                .unwrap_or(Value::Undefined);
        }
        self.property(target, &key.property_key())
    }

    fn set_computed(
        &mut self,
        target: &Value,
        key: &Value,
        value: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        match target {
            Value::Array(items) => match array_index(key) {
                Some(index) => {
                    let mut items = items.borrow_mut();
                    // Writing past the end extends with holes, which are `undefined` here.
                    // Real holes are a distinct concept the frontend refuses, so this stays a
                    // dense vector rather than growing a sparse representation it cannot yet
                    // observe the difference from.
                    if index >= items.len() {
                        items.resize(index + 1, Value::Undefined);
                    }
                    items[index] = value;
                    Ok(())
                }
                None => Err(ExecutionError::NotAnObject {
                    instruction,
                    value: "array with a non-index key",
                }),
            },
            Value::Object(object) => {
                object.borrow_mut().set(&key.property_key(), value);
                Ok(())
            }
            Value::Function(closure) => {
                closure
                    .properties
                    .borrow_mut()
                    .set(&key.property_key(), value);
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
        let Some(proto) = self.program.functions.get(function as usize) else {
            return Err(ExecutionError::UnknownFunction { function });
        };
        if proto.generator {
            // Calling a generator function does not run it: it captures the arguments and hands
            // back an object whose `next` runs the body up to each `yield`.
            let generator = self.build_generator(&closure, argc, instruction)?;
            self.stack.push(generator);
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

        // An arrow ignores the receiver entirely and uses what it captured, and the same holds
        // for `arguments`.
        let entering = match &closure.captured_this {
            Some(captured) => captured.clone(),
            None => this_value,
        };
        let received = match &closure.captured_arguments {
            Some(captured) => captured.clone(),
            None => received,
        };
        self.frames.push(Frame {
            function: self.function.take(),
            this_value: std::mem::replace(&mut self.this_value, entering),
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
        if target >= self.code().len() {
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
fn build_error(arguments: &[Value]) -> Value {
    let message = match arguments.first() {
        None | Some(Value::Undefined) => String::new(),
        Some(value) => value.to_display_string(),
    };

    let mut data = ObjectData::default();
    data.set("message", Value::String(message.clone()));
    data.set("name", Value::String("Error".to_owned()));
    data.set(
        "stack",
        Value::String(if message.is_empty() {
            "Error".to_owned()
        } else {
            format!("Error: {message}")
        }),
    );
    if let Some(Value::Object(options)) = arguments.get(1)
        && let Some(cause) = options.borrow().get("cause")
    {
        data.set("cause", cause.clone());
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
/// Not supported yet: the `replacer` argument and `toJSON` hooks, both of which call back into
/// the program and so need an interpreter this function does not have. Neither appears in the
/// corpus; when one does, this grows a `&mut Interpreter` and moves onto the type.
fn json_stringify(
    value: &Value,
    indent: &str,
    current: &str,
    seen: &mut Vec<usize>,
) -> Result<Option<String>, ExecutionError> {
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
        // Omitted rather than serialised: JSON has no way to spell any of these.
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
            let items = items.borrow();
            let parts = items
                .iter()
                .map(|item| {
                    // An element that is not representable becomes null rather than vanishing,
                    // because dropping it would renumber every element after it.
                    json_stringify(item, indent, &nested, guard.seen)
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
            let object = object.borrow();
            let mut parts = Vec::new();
            for (key, property) in object.entries() {
                // Internal bookkeeping -- an iterator's cursor, a well-known symbol -- is not an
                // own enumerable string property, so it is not part of the object's JSON.
                if key.starts_with(RESERVED_KEY_PREFIX) {
                    continue;
                }
                if let Some(part) = json_stringify(property, indent, &nested, guard.seen)? {
                    parts.push(format!("{}{colon}{part}", json_quote(key)));
                }
            }
            Some(if parts.is_empty() {
                "{}".to_owned()
            } else {
                format!("{{{open}{}{close}}}", parts.join(&separator))
            })
        }
        // Neither has own enumerable properties, so both serialise as the empty object -- which
        // is also what a real engine produces for them.
        Value::Generator(_) => Some("{}".to_owned()),
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
fn call_native(
    native: Native,
    receiver: &Value,
    arguments: &[Value],
) -> Result<Value, ExecutionError> {
    Ok(match native {
        Native::ArrayIsArray => Value::Boolean(matches!(arguments.first(), Some(Value::Array(_)))),
        Native::ObjectKeys => {
            let keys = match arguments.first() {
                Some(Value::Object(object)) => object
                    .borrow()
                    .keys()
                    .filter(|key| !key.starts_with(RESERVED_KEY_PREFIX))
                    .map(|key| Value::String(key.to_owned()))
                    .collect(),
                // `Object.keys([1,2])` is ["0","1"]: array indices are string keys too.
                Some(Value::Array(items)) => (0..items.borrow().len())
                    .map(|index| Value::String(index.to_string()))
                    .collect(),
                _ => Vec::new(),
            };
            Value::Array(Rc::new(RefCell::new(keys)))
        }
        Native::StringConstructor => Value::String(match arguments.first() {
            // `String(e)` on an error goes through `Error.prototype.toString`. Looking the
            // method up rather than sniffing for `name` and `message` means an ordinary object
            // that happens to have both is unaffected.
            Some(value @ Value::Object(_)) if has_error_to_string(value) => error_to_string(value),
            Some(value) => value.to_display_string(),
            None => String::new(),
        }),
        Native::ErrorToString => Value::String(error_to_string(receiver)),
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
                    _ => {}
                }
            }
            target
        }
        Native::ObjectGetPrototypeOf => match arguments.first() {
            Some(Value::Object(object)) => {
                object.borrow().prototype().cloned().unwrap_or(Value::Null)
            }
            Some(Value::Function(closure)) => closure
                .properties
                .borrow()
                .prototype()
                .cloned()
                .unwrap_or(Value::Null),
            _ => Value::Null,
        },
        // Peeled off by `dispatch_native`, which every call site goes through: walking an
        // iterable and applying a mapping function both need the interpreter, and this function
        // deliberately does not have one.
        Native::ArrayFrom => unreachable!("`Array.from` is dispatched by the interpreter"),
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
            Value::Array(items) => items.borrow_mut().pop().unwrap_or(Value::Undefined),
            _ => Value::Undefined,
        },
        Native::ArrayIndexOf | Native::ArrayIncludes => {
            let needle = arguments.first().cloned().unwrap_or(Value::Undefined);
            let found = match receiver {
                // `indexOf` uses strict equality, so NaN is never found and `-0` matches `0`.
                Value::Array(items) => items
                    .borrow()
                    .iter()
                    .position(|item| strict_equal(item, &needle)),
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
                Value::Array(Rc::new(RefCell::new(taken)))
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
            if !value.is_finite() {
                Value::String(number_string(value))
            } else {
                Value::String(format!("{value:.*}", digits as usize))
            }
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
        Native::JsonStringify => {
            let indent = json_indent(arguments.get(2));
            let mut seen = Vec::new();
            match json_stringify(
                arguments.first().unwrap_or(&Value::Undefined),
                &indent,
                "",
                &mut seen,
            )? {
                Some(text) => Value::String(text),
                // `JSON.stringify(undefined)` is `undefined`, not the string "undefined".
                None => Value::Undefined,
            }
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
                iterator.set("@@iterated", receiver.clone());
                iterator.set("@@index", Value::Number(0.0));
                iterator.set("next", Value::Native(Native::ArrayIteratorNext));
                // An iterator is itself iterable, which is what lets one be passed straight to
                // another `for-of` or to `Array.from`.
                iterator.set(
                    WellKnown::Iterator.property_key(),
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
        | Native::StringRepeat => {
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
                    Value::Array(Rc::new(RefCell::new(parts)))
                }
                Native::StringSlice => {
                    let characters: Vec<char> = text.chars().collect();
                    let resolve = |value: f64| -> usize {
                        if value < 0.0 {
                            (characters.len() as f64 + value).max(0.0) as usize
                        } else {
                            (value as usize).min(characters.len())
                        }
                    };
                    let start = resolve(arguments.first().map_or(0.0, Value::as_number));
                    let end = match arguments.get(1) {
                        None | Some(Value::Undefined) => characters.len(),
                        Some(value) => resolve(value.as_number()),
                    };
                    Value::String(characters[start.min(end)..end].iter().collect())
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
                "@@iterated",
                Value::Array(Rc::new(RefCell::new(characters))),
            );
            iterator.set("@@index", Value::Number(0.0));
            iterator.set("next", Value::Native(Native::StringIteratorNext));
            iterator.set(
                WellKnown::Iterator.property_key(),
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
                        Value::Array(Rc::new(RefCell::new(vec![key, value])))
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
        Native::CollectionValues => {
            let Some((entries, _)) = collection_state(receiver) else {
                return Ok(Value::Undefined);
            };
            let mut iterator = ObjectData::default();
            iterator.set("@@iterated", Value::Array(entries));
            iterator.set("@@index", Value::Number(0.0));
            iterator.set("next", Value::Native(Native::CollectionIteratorNext));
            iterator.set(
                WellKnown::Iterator.property_key(),
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
                    state.get("@@iterated").unwrap_or(Value::Undefined),
                    state
                        .get("@@index")
                        .unwrap_or(Value::Number(0.0))
                        .as_number() as usize,
                )
            };
            let Value::Array(entries) = &iterated else {
                return Ok(iteration_result(Value::Undefined, true));
            };
            // Skip holes left by deletion, and re-read the length each step so entries added
            // during iteration are seen -- both are observable and both are what a Set does.
            loop {
                let entry = entries.borrow().get(index).cloned();
                match entry {
                    None => {
                        state
                            .borrow_mut()
                            .set("@@index", Value::Number(index as f64));
                        return Ok(iteration_result(Value::Undefined, true));
                    }
                    Some(Value::Native(Native::Tombstone)) => index += 1,
                    Some(value) => {
                        state
                            .borrow_mut()
                            .set("@@index", Value::Number(index as f64 + 1.0));
                        return Ok(iteration_result(value, false));
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
                    state.get("@@iterated").unwrap_or(Value::Undefined),
                    state
                        .get("@@index")
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
                        .set("@@index", Value::Number(index as f64 + 1.0));
                    iteration_result(value, false)
                }
                None => iteration_result(Value::Undefined, true),
            }
        }
        Native::SetConstructor | Native::MapConstructor => Value::Undefined,
        Native::Error
        | Native::SymbolConstructor
        | Native::ArrayConstructor
        | Native::ObjectConstructor
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
        Value::Native(Native::Error) => match error_prototype {
            Some(prototype) => prototype.clone(),
            None => return false,
        },
        _ => return false,
    };

    let mut current = match value {
        Value::Object(object) => object.borrow().prototype().cloned(),
        _ => None,
    };
    while let Some(link) = current {
        if strict_equal(&link, &prototype) {
            return true;
        }
        current = match &link {
            Value::Object(object) => object.borrow().prototype().cloned(),
            _ => None,
        };
    }
    false
}

/// `key in target`.
fn has_property(target: &Value, key: &Value) -> bool {
    let name = key.property_key();
    match target {
        Value::Object(object) => object.borrow().get(&name).is_some(),
        Value::Array(items) => {
            if let Some(index) = array_index(key) {
                return index < items.borrow().len();
            }
            // Methods and `length` live on the prototype in a real engine; here the property
            // lookup answers for both, so `in` has to agree with it.
            name == "length" || !matches!(array_property(&name), Value::Undefined)
        }
        Value::Native(native) => native.property(&name).is_some(),
        _ => false,
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
        key if key == WellKnown::Iterator.property_key() => Value::Native(Native::StringValues),
        "toUpperCase" => Value::Native(Native::StringToUpperCase),
        "toLowerCase" => Value::Native(Native::StringToLowerCase),
        "includes" => Value::Native(Native::StringIncludes),
        "indexOf" => Value::Native(Native::StringIndexOf),
        "slice" => Value::Native(Native::StringSlice),
        "split" => Value::Native(Native::StringSplit),
        "trim" => Value::Native(Native::StringTrim),
        "startsWith" => Value::Native(Native::StringStartsWith),
        "endsWith" => Value::Native(Native::StringEndsWith),
        "repeat" => Value::Native(Native::StringRepeat),
        _ => Value::Undefined,
    }
}

/// The methods a Set or Map instance exposes, shared by property reads and `in`.
fn collection_property(is_map: bool, name: &str) -> Value {
    match (is_map, name) {
        (false, "add") => Value::Native(Native::SetAdd),
        (false, "has") => Value::Native(Native::SetHas),
        (false, "delete") => Value::Native(Native::SetDelete),
        (true, "set") => Value::Native(Native::MapSet),
        (true, "get") => Value::Native(Native::MapGet),
        (true, "has") => Value::Native(Native::MapHas),
        (true, "delete") => Value::Native(Native::MapDelete),
        (_, "values" | "keys" | "entries") => Value::Native(Native::CollectionValues),
        (_, key) if key == WellKnown::Iterator.property_key() => {
            Value::Native(Native::CollectionValues)
        }
        _ => Value::Undefined,
    }
}

/// The backing entries of a Set or Map, and whether it is a Map.
fn collection_state(receiver: &Value) -> Option<(Rc<RefCell<Vec<Value>>>, bool)> {
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
        "find" => Some(CallbackMethod::Find),
        "some" => Some(CallbackMethod::Some),
        "every" => Some(CallbackMethod::Every),
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
    Map,
    ForEach,
    Filter,
    /// `reduce`: the only one whose callback takes the accumulator, so it does not fit the
    /// `(item, index, array)` walk the rest share.
    Reduce,
    Find,
    Some,
    Every,
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
        "values" => Value::Native(Native::ArrayValues),
        key if key == WellKnown::Iterator.property_key() => Value::Native(Native::ArrayValues),
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
fn join(items: &[Value], separator: &str) -> String {
    items
        .iter()
        .map(|item| match item {
            Value::Undefined | Value::Null => String::new(),
            other => other.to_display_string(),
        })
        .collect::<Vec<_>>()
        .join(separator)
}

/// Every own property of a value, in enumeration order, symbol-keyed ones included.
///
/// This is what a spread copies, and it is deliberately not the same set `Object.keys` returns:
/// spread carries symbol keys across and `Object.keys` does not. An array or a string contributes
/// its indices, which is what makes `{ ...[1, 2] }` an object with keys "0" and "1"; a primitive
/// with nothing of its own contributes nothing, and so does `null` or `undefined`.
fn own_properties(value: &Value) -> Vec<(String, Value)> {
    match value {
        Value::Object(object) => object
            .borrow()
            .entries()
            .map(|(key, value)| (key.to_owned(), value.clone()))
            .collect(),
        Value::Array(items) => items
            .borrow()
            .iter()
            .enumerate()
            .map(|(index, item)| (index.to_string(), item.clone()))
            .collect(),
        Value::String(text) => text
            .chars()
            .enumerate()
            .map(|(index, character)| (index.to_string(), Value::String(character.to_string())))
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
        Some(radix) if radix.is_finite() && radix != 0.0 => radix as u32,
        _ => 0,
    };
    if !(radix == 0 || (2..=36).contains(&radix)) {
        return Value::Number(f64::NAN);
    }

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
                let (digits, rest) = self
                    .rest
                    .split_at_checked(4)
                    .ok_or(ExecutionError::InvalidJson)?;
                let code =
                    u32::from_str_radix(digits, 16).map_err(|_| ExecutionError::InvalidJson)?;
                self.rest = rest;
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
            return Ok(Value::Array(Rc::new(RefCell::new(items))));
        }
        loop {
            items.push(self.value()?);
            self.skip_whitespace();
            if self.eat(']') {
                return Ok(Value::Array(Rc::new(RefCell::new(items))));
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
        Value::Native(native) => format!("[Function: {native:?}]"),
        Value::Callback(method) => format!("[Function: {method:?}]"),
        Value::Generator(_) => "Object [Generator] {}".to_owned(),
        Value::Array(items) => {
            let Some(guard) = InspectGuard::enter(seen, Rc::as_ptr(items).cast::<()>()) else {
                return "[Circular *1]".to_owned();
            };
            if depth > INSPECT_DEPTH {
                return "[Array]".to_owned();
            }
            let items = items.borrow();
            if items.is_empty() {
                return "[]".to_owned();
            }
            let parts = items
                .iter()
                .map(|item| inspect_nested(item, depth + 1, guard.seen))
                .collect::<Vec<_>>();
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
    if let Some(Value::Array(entries)) = object.get_own("@@entries") {
        let is_map = matches!(object.get_own("@@map"), Some(&Value::Boolean(true)));
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
        .entries()
        // Reserved keys are the VM's own and symbol-keyed properties; node shows the latter as
        // `[Symbol(x)]: v`, which needs a key-to-symbol map this VM does not keep.
        .filter(|(key, _)| !key.starts_with(RESERVED_KEY_PREFIX))
        .map(|(key, value)| {
            format!(
                "{}: {}",
                inspect_key(key),
                inspect_nested(value, depth + 1, seen)
            )
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return "{}".to_owned();
    }
    format!("{{ {} }}", parts.join(", "))
}

/// A property name as a console writes it: bare when it is a plain identifier, quoted otherwise.
///
/// `$` is not in the set, which looks like an oversight and is not: node's own rule is
/// `/^[a-zA-Z_][a-zA-Z_0-9]*$/`, so `{ a$b: 1 }` prints with its key quoted and so does `{ 3: 1 }`.
/// Matching node exactly is what lets `scripts/differential.sh` diff console output at all.
fn inspect_key(key: &str) -> String {
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
fn index_key(key: &str) -> Option<u32> {
    let index: u32 = key.parse().ok()?;
    (index != u32::MAX && index.to_string() == key).then_some(index)
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
    let value = value.trim();
    if value.is_empty() {
        return 0.0;
    }
    value.parse::<f64>().unwrap_or(f64::NAN)
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
                name: Some("second".to_owned()),
                arrow: false,
                generator: false,
                parameter_count: 2,
                local_count: 2,
                code: vec![Instruction::LoadLocal(1), Instruction::Return],
                upvalues: Vec::new(),
            }],
        }
    }

    fn run(program: &Program) -> Result<Value, ExecutionError> {
        let mut host = TestHost { calls: Vec::new() };
        execute(program, &mut host, ExecutionLimits::default()).map(|result| result.value)
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

    /// The corpus asserts `Symbol.iterator in [1,2]`, so `in` and the property read have to
    /// agree about what an array exposes. They share one table for that reason.
    #[test]
    fn arrays_expose_the_same_properties_to_reads_and_to_in() {
        let items = Value::Array(Rc::new(RefCell::new(vec![Value::Number(1.0)])));
        for key in ["join", "values", "@@iterator", "length"] {
            assert!(
                has_property(&items, &Value::String(key.to_owned())),
                "`{key} in array` must be true"
            );
        }
        assert!(has_property(&items, &Value::Number(0.0)), "index 0 present");
        assert!(!has_property(&items, &Value::Number(1.0)), "index 1 absent");
    }

    /// A symbol keys a property under its reserved slot, not under its printable description.
    #[test]
    fn a_symbol_key_is_not_its_display_string() {
        let symbol = Value::Symbol(Symbol::WellKnown(WellKnown::Iterator));
        assert_eq!(symbol.property_key(), "@@iterator");
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
            &array_property("@@iterator")
        ));
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
        let with_cause = build_error(&[
            Value::String("m".to_owned()),
            object(&[("cause", Value::Undefined)]),
        ]);
        let Value::Object(data) = &with_cause else {
            panic!("construct_error returns an object");
        };
        assert_eq!(data.borrow().get("cause"), Some(Value::Undefined));

        let without = build_error(&[Value::String("m".to_owned())]);
        let Value::Object(data) = &without else {
            panic!("construct_error returns an object");
        };
        assert_eq!(data.borrow().get("cause"), None, "no options, no cause");
    }

    /// A cause is carried by identity: the error holds the same object the caller passed.
    #[test]
    fn error_cause_keeps_the_identity_of_the_value_given() {
        let inner = object(&[("marker", Value::Number(1.0))]);
        let outer = build_error(&[
            Value::String("outer".to_owned()),
            object(&[("cause", inner.clone())]),
        ]);
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
            let Value::Object(data) = build_error(&arguments) else {
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
        assert_eq!(Value::Native(Native::Error).type_of(), "function");
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
            Err(ExecutionError::NotConstructable {
                value: "number",
                ..
            })
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
        let entries = vec![Value::Array(Rc::new(RefCell::new(vec![
            key.clone(),
            Value::String("found".to_owned()),
        ])))];

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
            Err(ExecutionError::NotAnObject {
                value: "number",
                ..
            })
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
                    name: Some("outer".to_owned()),
                    arrow: false,
                    generator: false,
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
                    name: Some("inner".to_owned()),
                    arrow: false,
                    generator: false,
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
                    name: Some("outer".to_owned()),
                    arrow: false,
                    generator: false,
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
                    name: Some("read".to_owned()),
                    arrow: false,
                    generator: false,
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
                name: None,
                arrow: false,
                generator: false,
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
        assert!(matches!(
            run(&program),
            Err(ExecutionError::NotCallable {
                value: "number",
                ..
            })
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
                name: Some("forever".to_owned()),
                arrow: false,
                generator: false,
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
        assert!(matches!(
            execute(&program, &mut host, limits),
            Err(ExecutionError::CallDepthExceeded { limit: 8 })
        ));
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
                    name: Some("write".to_owned()),
                    arrow: false,
                    generator: false,
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
                    name: Some("read".to_owned()),
                    arrow: false,
                    generator: false,
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
        let result = execute(&program, &mut host, ExecutionLimits::default()).unwrap();
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
        let result = execute(&program, &mut host, ExecutionLimits::default()).unwrap();
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
        let result = execute(&program, &mut host, ExecutionLimits::default()).unwrap();
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
        execute(&program, &mut host, ExecutionLimits::default()).unwrap();
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
            &program,
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
                code: vec![
                    Instruction::LoadLocal(0),
                    Instruction::Const(0),
                    Instruction::Binary(BinaryOp::Multiply),
                    Instruction::StoreLocal(1),
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
        assert_eq!(result.exports[0].value, Value::Number(42.0));
    }

    fn json(value: &Value) -> Option<String> {
        json_stringify(value, "", "", &mut Vec::new()).expect("no cycle in this fixture")
    }

    fn array(items: &[Value]) -> Value {
        Value::Array(Rc::new(RefCell::new(items.to_vec())))
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
    fn json_skips_internal_properties() {
        // `@@`-prefixed entries are the VM's own bookkeeping -- an iterator cursor, a well-known
        // symbol -- and are not own enumerable string properties of the object.
        let value = object(&[
            ("@@index", Value::Number(0.0)),
            ("real", Value::Number(1.0)),
        ]);
        assert_eq!(json(&value).as_deref(), Some(r#"{"real":1}"#));
    }

    #[test]
    fn json_escapes_strings() {
        let value = Value::String("a\"b\\c\nd\u{1}".to_owned());
        assert_eq!(json(&value).as_deref(), Some(r#""a\"b\\c\nd\u0001""#));
    }

    #[test]
    fn json_empty_containers_stay_on_one_line_when_indented() {
        let value = object(&[("a", array(&[])), ("b", object(&[]))]);
        let text = json_stringify(&value, "  ", "", &mut Vec::new()).expect("no cycle");
        assert_eq!(text.as_deref(), Some("{\n  \"a\": [],\n  \"b\": {}\n}"));
    }

    #[test]
    fn json_indents_nested_containers() {
        let value = object(&[("a", array(&[Value::Number(1.0)]))]);
        let text = json_stringify(&value, "  ", "", &mut Vec::new()).expect("no cycle");
        assert_eq!(text.as_deref(), Some("{\n  \"a\": [\n    1\n  ]\n}"));
    }

    #[test]
    fn json_rejects_a_value_that_contains_itself() {
        let items = Rc::new(RefCell::new(Vec::new()));
        items.borrow_mut().push(Value::Array(Rc::clone(&items)));
        assert!(matches!(
            json_stringify(&Value::Array(items), "", "", &mut Vec::new()),
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
    fn a_symbols_key_is_reserved_so_a_string_cannot_spell_it() {
        let symbol = Symbol::Unique(Rc::new(UniqueSymbol {
            serial: 7,
            description: None,
        }));
        assert!(symbol.property_key().starts_with(RESERVED_KEY_PREFIX));
        assert!(
            Symbol::WellKnown(WellKnown::Iterator)
                .property_key()
                .starts_with(RESERVED_KEY_PREFIX)
        );
    }

    #[test]
    fn spread_reads_every_own_property_including_symbol_keyed_ones() {
        let symbol = Symbol::Unique(Rc::new(UniqueSymbol {
            serial: 1,
            description: None,
        }));
        let source = object(&[
            ("plain", Value::Number(1.0)),
            (&symbol.property_key(), Value::Number(2.0)),
        ]);
        let copied = own_properties(&source);
        assert_eq!(copied.len(), 2, "a spread carries symbol keys across");
        assert_eq!(copied[0].0, "plain");
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
        let items = Rc::new(RefCell::new(Vec::new()));
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
    fn a_symbol_shows_its_description_rather_than_its_reserved_key() {
        // The key is `@@iterator`, which is an implementation detail and not what a console says.
        let symbol = Value::Symbol(Symbol::WellKnown(WellKnown::Iterator));
        assert_eq!(inspect(&symbol), "Symbol(Symbol.iterator)");
        assert_eq!(symbol.property_key(), "@@iterator");
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
}
