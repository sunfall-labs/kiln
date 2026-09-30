use super::*;

/// Identifies one realm within an execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct RealmId(pub u32);

impl RealmId {
    /// The realm an execution starts in.
    pub const PRIMARY: RealmId = RealmId(0);
}

/// A JavaScript scalar value represented by the interpreter.
#[derive(Clone, Debug)]
pub enum Value {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(StringUnits),
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
    Native(Native, RealmId),
    /// A suspended generator: a frame that can stop and resume, which nothing else here needs.
    Generator(Rc<GeneratorData>),
    Promise(Rc<PromiseData>),
    RegExp(Rc<RegExpData>),
    /// A date: milliseconds since the epoch, behind an `Rc` because two dates built from the same
    /// instant are still two objects and `===` has to say so.
    Date(Rc<DateData>),
    /// One of the eight `Temporal` types, held opaquely by the linked calendar engine.
    ///
    /// Its own value kind rather than an object with reserved state, for the same reason `Date` is
    /// one: every operation on it has to reach the engine, and a `Temporal.PlainDate` that could be
    /// confused with an ordinary object would answer field reads from the wrong place. Behind an
    /// `Rc` for identity, not for mutation -- every `Temporal` type is immutable, so nothing here
    /// ever writes through it.
    Temporal(Rc<TemporalData>),
    /// A built `Intl` formatter, held opaquely by the linked internationalization engine.
    ///
    /// Its own value kind rather than an object with reserved slots, for the same reason
    /// `Temporal` is one: the state is the engine's, rebuilding it per call would be both
    /// slow and observably different, and the brand check that stops
    /// `NumberFormat.prototype.format` running on a `Collator` needs a shape to check.
    Intl(Rc<intl::IntlValue>),
    /// An arbitrary-precision integer. A separate numeric type rather than a wider `Number`,
    /// because the language keeps them apart on purpose: mixing one with a number in arithmetic
    /// is a TypeError, and precision that does not silently stop is the reason to reach for it.
    BigInt(Rc<num_bigint::BigInt>),
    /// The `resolve` or `reject` handed to a promise executor.
    ///
    /// A function bound to one promise, which no other value kind here can express: a `Native`
    /// carries no state and a `Closure` carries bytecode. The flag says which of the two it is.
    Settler(Rc<PromiseData>, bool),
    /// The `GetCapabilitiesExecutor` a `NewPromiseCapability(C)` hands to `C`.
    ///
    /// `Promise.resolve.call(C, v)` is defined as constructing a `C` and *catching the two
    /// functions its executor is given*, which is the only way a subclass with its own
    /// constructor can take part. That needs a callable holding state -- the pair, once they
    /// arrive -- and `Settler` was the only one the machine had, so every static that owes a
    /// capability ignored `this` and built an intrinsic promise instead.
    Capability(Rc<CapabilityRecord>),
    /// A `Promise.all` Resolve Element Function, or the `allSettled`/`any` equivalent.
    ///
    /// Handed to a program -- `then` receives it -- so it has to be a value, and it carries the
    /// index it fills and the walk it belongs to, which neither `Native` nor `Closure` can hold.
    Element(Rc<ElementFunction>),
    /// An array. Separate from `Object` rather than an object with numeric keys: `Array.isArray`
    /// has to be able to tell them apart, `length` is not an ordinary property, and a dense
    /// `Vec` is the representation that makes indexing the common case it is.
    Array(Rc<ArrayData>),
    /// A realm-local facade over one node of a sealed shared graph (ADR 0073). The facade owns a
    /// strong graph handle; the arena contains no realm value or prototype pointer.
    Shared(Rc<SharedFacade>),
    /// A built-in that takes a callback, invoked by the VM rather than by `call_native`.
    Callback(CallbackMethod, RealmId),
    /// A view over a byte buffer: `Uint8Array` and its siblings.
    ///
    /// Separate from `Array` because the difference is the point: the elements are bytes in a
    /// shared buffer with a fixed width and a fixed length, two views can overlap, and writing
    /// through one is visible through the other. An array of numbers cannot express any of that.
    TypedArray(Rc<TypedArrayData>),
    /// An `ArrayBuffer`: bytes with no interpretation, which only a view gives them.
    ArrayBuffer(Rc<BufferData>),
    /// A `DataView` over one.
    DataView(Rc<DataViewData>),
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
    /// A callable that a `ShadowRealm` handed out: see [`WrappedFunction`].
    ///
    /// Its own variant rather than a `Bound` with a clever target, because the two differ in the
    /// one place that matters: a bound function *is* its target with arguments fixed, and passes
    /// values through untouched, while this one is a membrane -- it re-wraps in both directions
    /// and refuses anything that is neither a primitive nor callable. Collapsing them would
    /// silently leak a cross-realm object, which is the whole thing the wrapper exists to stop.
    Wrapped(Rc<WrappedFunction>),
    /// A module-scoped CommonJS loader bound to already-resolved graph edges.
    CommonJsRequire(Rc<CommonJsRequire>),
    CommonJsResolve(Rc<CommonJsRequire>),
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

/// Which reading of a value `ToPrimitive` is asking for.
///
/// The specification has three hints, not two, and the third is not decoration: `Date` is the one
/// built-in whose `@@toPrimitive` answers differently for "default" than for "number", which is
/// exactly why `date + date` concatenates two date strings while `date - date` subtracts two
/// instants. Collapsing "default" into either of the others gets one of those two wrong.
///
/// For every other value the three coincide, which is why a two-valued flag went unnoticed here
/// for so long, and why the spelling is what the C backend emits too -- a program that branches on
/// the hint it was handed cannot tell the two engines apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrimitiveHint {
    /// Prefer a string: `toString` before `valueOf`.
    String,
    /// Prefer a number: `valueOf` before `toString`.
    Number,
    /// State no preference. Ordinary values read this exactly as `Number`; a `Date` does not.
    Default,
}

impl PrimitiveHint {
    /// The hint as `@@toPrimitive` receives it.
    pub(super) fn spelling(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Default => "default",
        }
    }
}

// Rust-side equality for interpreter values is JavaScript strict equality. Keeping this manual
// rather than deriving it prevents `Rc<T>` payload equality from turning object identity into a
// structural comparison, and `strict_equal` remains exhaustive so every new representation must
// choose its semantics before the crate compiles.
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        strict_equal(self, other)
    }
}

/// The specifier a Node builtin namespace value stands for.
///
/// The inverse of the builtin namespace projection, and the only thing that lets a `Native` reach its
/// own entry in the shared ABI table -- which is where the list of names it publishes lives.
pub(super) fn node_builtin_specifier(native: Native) -> Option<&'static str> {
    Some(match native {
        Native::NodeAssert => "node:assert",
        Native::NodeAsyncHooks => "node:async_hooks",
        Native::NodeBuffer => "node:buffer",
        Native::NodeConsole => "node:console",
        Native::NodeCrypto => "node:crypto",
        Native::NodeDiagnosticsChannel => "node:diagnostics_channel",
        Native::NodeDns => "node:dns",
        Native::NodeEvents => "node:events",
        Native::NodeFs => "node:fs",
        Native::NodeFsPromises => "node:fs/promises",
        Native::NodeHttp => "node:http",
        Native::NodeHttps => "node:https",
        Native::NodeHttp2 => "node:http2",
        Native::NodeModule => "node:module",
        Native::NodeNet => "node:net",
        Native::NodeOs => "node:os",
        Native::NodePath => "node:path",
        Native::NodePerfHooks => "node:perf_hooks",
        Native::NodeQueryString => "node:querystring",
        Native::NodeStream => "node:stream",
        Native::NodeStringDecoder => "node:string_decoder",
        Native::NodeTty => "node:tty",
        Native::NodeUrl => "node:url",
        Native::NodeUtil => "node:util",
        Native::NodeWorkerThreads => "node:worker_threads",
        Native::NodeZlib => "node:zlib",
        _ => return None,
    })
}

impl Value {
    /// The string a value becomes when used as a property key.
    ///
    /// Distinct from `to_display_string`: a symbol keys a property under its reserved slot, not
    /// under its human-readable description, and conflating the two would make
    /// `obj[Symbol.iterator]` and `obj["Symbol(Symbol.iterator)"]` the same property.
    pub fn property_key_lossy(&self) -> String {
        match self {
            Self::Symbol(symbol) => symbol.property_key(),
            other => other.to_display_string(),
        }
    }

    /// Lossless `ToPropertyKey` spelling for ordinary string values. Symbols retain their
    /// reserved internal name; all other values use the already-computed language string.
    pub(super) fn property_key_units_checked(&self) -> Result<StringUnits, ExecutionError> {
        match self {
            Self::Symbol(symbol) => Ok(StringUnits::from(symbol.property_key())),
            Self::String(value) => Ok(value.clone()),
            other => other.to_string_units_checked(),
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
            Self::Object(_) | Self::Shared(_) => "object",
            // A method table stands for a prototype *object*, and `typeof Object.prototype` is
            // "object". `Function.prototype` is the exception the specification makes: it is a
            // function that does nothing.
            Self::Native(Native::PrototypeOf(PrototypeTable::Function), _) => "function",
            Self::Native(Native::PrototypeOf(_), _) => "object",
            Self::Native(
                Native::GeneratorFunctionPrototype | Native::AsyncGeneratorFunctionPrototype,
                _,
            ) => "object",
            // A namespace is an object holding functions, not a function: `Math` and `JSON`
            // and `Reflect` cannot be called, and a library that feature-detects with
            // `typeof Math === "object"` has to find one.
            Self::Native(Native::Math | Native::Json | Native::Reflect | Native::Atomics, _)
            | Self::Native(Native::Intl(intl::Op::Namespace | intl::Op::Prototype(_)), _)
            // `Temporal` and `Temporal.Now` are namespaces for the same reason, and each of the
            // eight `Temporal.X.prototype` values is an ordinary object. None of the three can be
            // called. The native backend already answers "object" for the two namespaces, so
            // "function" here was not merely wrong -- it was the two engines disagreeing
            // somewhere no probe happened to look.
            | Self::Native(
                Native::Temporal(
                    temporal::Op::Namespace | temporal::Op::Now | temporal::Op::Prototype(_),
                ),
                _,
            )
            // `performance` is a `Performance` object holding `now`, not something a program
            // calls. node answers "object" for both `globalThis.performance` and
            // `require("node:perf_hooks").performance`, and the native backend answers "object"
            // too; this said "function", which is the answer a library's
            // `typeof performance === "object"` guard walks past on its way to installing a shim
            // it did not need -- and it made the same bytecode read two ways in the two engines.
            | Self::Native(Native::Performance, _)
            | Self::Intl(_) => "object",
            // A Node builtin's default export is the same shape of thing, and the same
            // feature-detection depends on it: `typeof http === "object"` is how a library
            // decides it is on Node at all. Which of these is an object and which is callable is
            // not a judgement call - it was measured against node v25.9.0, one probe per
            // specifier, and only these answered "object". The four that answered "function" are
            // deliberately absent and must stay absent: node's `assert`, `events`, `module` and
            // `stream` defaults really are callable (`assert(cond)`, the `EventEmitter`
            // constructor, and the `Stream` constructor), so listing them here would trade one
            // wrong answer for another.
            Self::Native(
                Native::NodeAsyncHooks
                | Native::NodeBuffer
                | Native::NodeConsole
                | Native::NodeCrypto
                | Native::NodeDiagnosticsChannel
                | Native::NodeDns
                | Native::NodeFs
                | Native::NodeFsPromises
                | Native::NodeHttp
                | Native::NodeHttp2
                | Native::NodeHttps
                | Native::NodeNet
                | Native::NodeOs
                | Native::NodePath
                | Native::NodePerfHooks
                | Native::NodeQueryString
                | Native::NodeStringDecoder
                | Native::NodeTty
                | Native::NodeUrl
                | Native::NodeUtil
                | Native::NodeWorkerThreads
                | Native::NodeZlib,
                _,
            ) => "object",
            Self::Native(_, _) | Self::Callback(_, _) | Self::Bound(_) | Self::Wrapped(_) => {
                "function"
            }
            Self::CommonJsRequire(_) | Self::CommonJsResolve(_) => "function",
            // `typeof` has no trap: a proxy reports whatever its target is, which is what makes
            // a proxied function still read as a function.
            Self::Proxy(proxy) => proxy.target.type_of(),
            Self::TypedArray(_) | Self::ArrayBuffer(_) | Self::DataView(_) => "object",
            Self::Array(_)
            | Self::Generator(_)
            | Self::Promise(_)
            | Self::RegExp(_)
            | Self::Date(_)
            | Self::Temporal(_) => "object",
            Self::BigInt(_) => "bigint",
            // A settler is a function the program can call, hold and pass on, so it says so.
            Self::Settler(..) | Self::Capability(_) | Self::Element(_) => "function",
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
            Self::Shared(facade) => match facade.handle.graph().node(facade.handle.index()) {
                Some(kiln_shared_graph::Node::Array { .. }) => "array",
                _ => "object",
            },
            Self::Native(Native::PrototypeOf(PrototypeTable::Function), _) => "function",
            Self::Native(Native::PrototypeOf(_), _) => "object",
            Self::Native(
                Native::GeneratorFunctionPrototype | Native::AsyncGeneratorFunctionPrototype,
                _,
            ) => "object",
            // A namespace is an object holding functions, not a function: `Math` and `JSON`
            // and `Reflect` cannot be called, and a library that feature-detects with
            // `typeof Math === "object"` has to find one.
            Self::Native(Native::Math | Native::Json | Native::Reflect | Native::Atomics, _)
            | Self::Native(Native::Intl(intl::Op::Namespace | intl::Op::Prototype(_)), _)
            // A diagnostic calls `performance` what `typeof` calls it, for the same reason.
            | Self::Native(Native::Performance, _)
            | Self::Intl(_) => "object",
            // A Node builtin's default export is the same shape of thing, and the same
            // feature-detection depends on it: `typeof http === "object"` is how a library
            // decides it is on Node at all. Which of these is an object and which is callable is
            // not a judgement call - it was measured against node v25.9.0, one probe per
            // specifier, and only these answered "object". The four that answered "function" are
            // deliberately absent and must stay absent: node's `assert`, `events`, `module` and
            // `stream` defaults really are callable (`assert(cond)`, the `EventEmitter`
            // constructor, and the `Stream` constructor), so listing them here would trade one
            // wrong answer for another.
            Self::Native(
                Native::NodeAsyncHooks
                | Native::NodeBuffer
                | Native::NodeConsole
                | Native::NodeCrypto
                | Native::NodeDiagnosticsChannel
                | Native::NodeDns
                | Native::NodeFs
                | Native::NodeFsPromises
                | Native::NodeHttp
                | Native::NodeHttp2
                | Native::NodeHttps
                | Native::NodeNet
                | Native::NodeOs
                | Native::NodePath
                | Native::NodePerfHooks
                | Native::NodeQueryString
                | Native::NodeStringDecoder
                | Native::NodeTty
                | Native::NodeUrl
                | Native::NodeUtil
                | Native::NodeWorkerThreads
                | Native::NodeZlib,
                _,
            ) => "object",
            Self::Native(_, _) | Self::Callback(_, _) | Self::Bound(_) | Self::Wrapped(_) => {
                "function"
            }
            Self::CommonJsRequire(_) | Self::CommonJsResolve(_) => "function",
            Self::Proxy(proxy) => proxy.target.type_name(),
            Self::TypedArray(view) => view.kind.name(),
            Self::ArrayBuffer(_) => "arraybuffer",
            Self::DataView(_) => "dataview",
            Self::Array(_) => "array",
            Self::Generator(_) => "generator",
            Self::Promise(_) => "promise",
            Self::RegExp(_) => "regexp",
            Self::Date(_) => "date",
            Self::Temporal(built) => built.kind().tag(),
            Self::BigInt(_) => "bigint",
            Self::Settler(..) | Self::Capability(_) | Self::Element(_) => "function",
            Self::Symbol(_) => "symbol",
            Self::Cell(_) => "cell",
        }
    }

    pub fn from_number_bits(bits: u64) -> Self {
        Self::Number(f64::from_bits(bits))
    }

    pub fn as_number(&self) -> f64 {
        // A wrapper converts to what it boxes: `Object(1) + 1` is 2, `+new String("3")` is 3,
        // and `Number(Number.prototype)` is 0. Nothing else about the object takes part.
        if let Some(boxed) = wrapped_primitive(self) {
            return boxed.as_number();
        }
        match self {
            Self::Undefined => f64::NAN,
            Self::Null => 0.0,
            Self::Boolean(value) => f64::from(*value),
            Self::Number(value) => *value,
            Self::String(value) => parse_number_units(value),
            // `Number(fn)` is NaN in JavaScript: a function has no numeric value, and ToNumber
            // on it goes through ToPrimitive, which cannot produce one.
            Self::Function(_) => f64::NAN,
            // `Number({})` is NaN: ToPrimitive on a plain object yields "[object Object]",
            // which does not parse as a number.
            Self::Object(_)
            | Self::Shared(_)
            | Self::Native(_, _)
            | Self::Callback(_, _)
            | Self::Generator(_)
            | Self::Promise(_)
            | Self::RegExp(_)
            | Self::Settler(..)
            | Self::Capability(_)
            | Self::Element(_)
            | Self::Bound(_)
            | Self::Wrapped(_)
            | Self::CommonJsRequire(_)
            | Self::CommonJsResolve(_) => f64::NAN,
            // Coercion is defined on the target: the traps that could change it are `get` on
            // `valueOf` and `toString`, which this pure path cannot call anyway.
            Self::Proxy(proxy) => proxy.target.as_number(),
            // `Number(new Uint8Array([5]))` is 5 and `Number(new Uint8Array([1,2]))` is NaN:
            // ToPrimitive joins, and only a one-element join parses as a number.
            Self::TypedArray(view) => {
                join(&view.items(), ",").map_or(f64::NAN, |text| parse_number(&text))
            }
            Self::ArrayBuffer(_) | Self::DataView(_) => f64::NAN,
            // Temporal objects do not have a numeric primitive conversion. Operations that
            // accept one ask the linked engine explicitly rather than passing through Number.
            Self::Temporal(_) => f64::NAN,
            // Nor does an `Intl` formatter: `+new Intl.NumberFormat()` is NaN, the same as
            // for any object with no numeric primitive.
            Self::Intl(_) => f64::NAN,
            // A date converts to its instant, which is what makes `end - start` a duration.
            Self::Date(date) => date.time.get(),
            // Only where a number is explicitly asked for; arithmetic refuses the mix.
            Self::BigInt(value) => {
                num_traits::ToPrimitive::to_f64(value.as_ref()).unwrap_or(f64::NAN)
            }
            // `Number(Symbol())` is a TypeError; NaN is the closest thing available without one.
            Self::Symbol(_) => f64::NAN,
            // `Number([])` is 0 and `Number([2])` is 2: ToPrimitive joins, then parses.
            // A join too long to build cannot be parsed as a number either. The specification
            // raises the RangeError from the join and this signature has nowhere to put it, so
            // the refusal shows up as NaN -- which fails such a test rather than passing it, and
            // is what the alternative (killing the process) reports for nothing at all.
            Self::Array(items) => {
                join(&items.borrow(), ",").map_or(f64::NAN, |text| parse_number(&text))
            }
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
            | Self::Shared(_)
            | Self::Native(_, _)
            | Self::Callback(_, _)
            | Self::Array(_)
            | Self::Generator(_)
            | Self::Promise(_)
            | Self::RegExp(_)
            | Self::Date(_)
            | Self::Settler(..)
            | Self::Capability(_)
            | Self::Element(_)
            | Self::Bound(_)
            | Self::Wrapped(_)
            | Self::CommonJsRequire(_)
            | Self::CommonJsResolve(_)
            | Self::Proxy(_)
            | Self::TypedArray(_)
            | Self::ArrayBuffer(_)
            | Self::DataView(_)
            | Self::Temporal(_)
            | Self::Intl(_)
            | Self::Symbol(_) => true,
            Self::BigInt(value) => !num_traits::Zero::is_zero(value.as_ref()),
            Self::Cell(_) => unreachable!("a cell is never used as a value"),
        }
    }

    pub fn is_nullish(&self) -> bool {
        matches!(self, Self::Undefined | Self::Null)
    }

    pub fn to_display_string(&self) -> String {
        // A wrapper stringifies as what it boxes: `String(new String("x"))` is "x" and
        // `String(String.prototype)` is "". The box is invisible to every conversion; only
        // `Object.prototype.toString` reports it.
        if let Some(boxed) = wrapped_primitive(self) {
            return boxed.to_display_string();
        }
        match self {
            Self::Undefined => "undefined".to_owned(),
            Self::Null => "null".to_owned(),
            Self::Boolean(value) => value.to_string(),
            Self::Number(value) => number_string(*value),
            // This method is explicitly the host/diagnostic rendering API. Language string
            // coercion uses `to_string_units` below, so a lone unit is only replaced here at the
            // final display boundary where Rust's scalar `String` is required.
            Self::String(value) => value.to_lossy_string(),
            // `Function.prototype.toString`: the text the program wrote. The span and the source
            // both travel on the program the closure already carries, so this is a slice rather
            // than a reconstruction -- there is no spelling to invent and none to get wrong.
            Self::Function(closure) => closure.source_text().to_owned(),
            // What `String({})` produces. `console.log` in a real console prints the contents
            // instead, which needs a formatter the host side does not have yet.
            Self::Object(_) | Self::Shared(_) => "[object Object]".to_owned(),
            // The same name a console shows. `String(f)` in node is the source text, which a
            // backend does not have; naming the function beats an invented body, and naming it
            // the way the program does beats naming it the way the implementation does.
            // A built-in has no source text to answer with, and the specification says so: it
            // must produce the NativeFunction form instead. Getting this backwards is worse than
            // saying nothing, because feature detection is written as
            // `/native code/.test(String(x))` -- a built-in that reported a body would be read as
            // a polyfill, and a user function that reported `[native code]` as already-native.
            Self::Native(native, _) => match namespace_name(native) {
                // A namespace is an object that happens to hold functions, and `String` of an
                // object is its `toString` tag. `typeof Math` already answered `"object"`; this
                // is the same fact reaching the other conversion.
                Some(name) => format!("[object {name}]"),
                None => native_code_source(&native_js_name(native)),
            },
            Self::Callback(method, _) => {
                native_code_source(&builtin_js_name(&format!("{method:?}")))
            }
            Self::Generator(_) => "[object Generator]".to_owned(),
            Self::Promise(_) => "[object Promise]".to_owned(),
            // `String(/a/g)` is the literal as written, which is what `toString` returns.
            Self::RegExp(regexp) => format!(
                "/{}/{}",
                regexp.source.to_lossy_string(),
                canonical_regexp_flags(&regexp.flags)
            ),
            Self::Date(date) => date_to_string(date.time.get()),
            Self::Temporal(built) => built
                .value()
                .to_temporal_string(&kiln_temporal::Options::default())
                .unwrap_or_else(|_| format!("[object {}]", built.kind().tag())),
            // `Intl.Locale` stringifies as its tag, which is what `toString` returns; the
            // formatters have no string form of their own and fall back to the object tag.
            Self::Intl(value) => match value.as_ref() {
                intl::IntlValue::Locale(tag) => tag.clone(),
                other => format!("[object {}]", other.service().tag()),
            },
            // No `n` suffix: that is the literal's spelling, not the value's.
            Self::BigInt(value) => value.to_string(),
            Self::Settler(..)
            | Self::Capability(_)
            | Self::Element(_)
            | Self::Bound(_)
            | Self::Wrapped(_) => "function () { [native code] }".to_owned(),
            Self::CommonJsRequire(_) => "function require() { [native code] }".to_owned(),
            Self::CommonJsResolve(_) => "function resolve() { [native code] }".to_owned(),
            // A proxy of a *function* has no source text of its own, and answering with the
            // target's is a wrong answer rather than a missing one: `Function.prototype.toString`
            // requires the NativeFunction shape, so a proxy that reported `() => {}` reads as
            // the arrow it wraps to anything that parses this. Node answers the anonymous form
            // for every function proxy, whatever it wraps. A proxy of anything else still
            // stringifies as what it wraps, which is what makes `String(new Proxy([1, 2], {}))`
            // "1,2".
            Self::Proxy(proxy) if is_callable(&proxy.target) => native_code_source(""),
            Self::Proxy(proxy) => proxy.target.to_display_string(),
            // Like an array: `String(new Uint8Array([1, 2]))` is "1,2".
            Self::TypedArray(view) => join(&view.items(), ",").unwrap_or_default(),
            Self::ArrayBuffer(buffer) => if buffer.shared {
                "[object SharedArrayBuffer]"
            } else {
                "[object ArrayBuffer]"
            }
            .to_owned()
            .into(),
            Self::DataView(_) => "[object DataView]".to_owned(),
            Self::Symbol(symbol) => format!("Symbol({})", symbol.description()),
            // `String([1,2])` is "1,2": arrays stringify as a comma join, not as [object Array].
            // `String(array)` is defined in terms of `join`, which can refuse a result too long
            // to build -- and this signature, like `as_number`'s, has nowhere to report it. The
            // empty string is the visibly wrong answer that a test asserting the throw fails on;
            // `array.join()` itself, which can return a result, raises the RangeError.
            Self::Array(items) => join(&items.borrow(), ",").unwrap_or_default(),
            Self::Cell(_) => unreachable!("a cell is never used as a value"),
        }
    }

    /// Fallible lossless `ToString` used by language operations that can propagate a string-size
    /// refusal. The conversion itself is already complete by the time this method is called;
    /// object-to-primitive side effects are owned by `Vm::as_primitive`.
    pub(super) fn to_string_units_checked(&self) -> Result<StringUnits, ExecutionError> {
        match self {
            Self::String(value) => Ok(value.clone()),
            Self::Array(items) => join_units(&items.borrow(), &StringUnits::from(",")),
            Self::TypedArray(view) => join_units(&view.items(), &StringUnits::from(",")),
            Self::RegExp(regexp) => Ok(regexp_string_units(regexp)),
            Self::Proxy(proxy) if !is_callable(&proxy.target) => {
                proxy.target.to_string_units_checked()
            }
            Self::Intl(value) => match value.as_ref() {
                intl::IntlValue::Locale(tag) => Ok(StringUnits::from(tag.as_str())),
                _ => Ok(StringUnits::from(self.to_display_string())),
            },
            _ => Ok(StringUnits::from(self.to_display_string())),
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
            Constant::WellKnownSymbol(symbol) => {
                Self::Symbol(crate::symbol::Symbol::WellKnown(symbol))
            }
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
            other => Self::String(inspect(&other).into()),
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
            HostValue::String(value) => Self::String(value.into()),
        }
    }
}
