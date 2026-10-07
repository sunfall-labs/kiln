//! Global binding names shared by source optimization, IR lowering, and both engines.
//! Discriminant order is part of native artifact identity and must remain stable.

/// A supplied global binding, with one canonical spelling shared across compiler stages.
/// Source reads still use the mutable global object; this declaration does not freeze them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GlobalBuiltin {
    Error,
    Symbol,
    String,
    GlobalThis,
    Set,
    Map,
    Array,
    Object,
    Json,
    // Appended rather than inserted: the discriminant is hashed into the artifact cache key, so
    // renumbering the existing ones would invalidate every cached native build.
    Math,
    Number,
    Boolean,
    /// `NaN` and `Infinity` are values rather than callables, but they reach the program the same
    /// way -- as a name the compiler resolves to something the runtime supplies.
    NotANumber,
    Infinity,
    ParseInt,
    EncodeUri,
    EncodeUriComponent,
    DecodeUri,
    DecodeUriComponent,
    ParseFloat,
    IsNaN,
    IsFinite,
    TypeError,
    RangeError,
    ReferenceError,
    SyntaxError,
    UriError,
    SetTimeout,
    SetInterval,
    ClearTimeout,
    ClearInterval,
    QueueMicrotask,
    Promise,
    RegExp,
    Date,
    WeakMap,
    WeakSet,
    Performance,
    Reflect,
    BigInt,
    /// `process`, in the minimal shape a bundle actually reads.
    Process,
    /// Node's global byte container. The interpreter installs the Node-compatible surface only;
    /// native Node execution remains unadvertised until it shares that runtime adapter.
    Buffer,
    /// WHATWG URL, used by both web-compatible programs and Node servers.
    Url,
    /// `Function`, reached for its `prototype` and its `toString` rather than to construct.
    Function,
    Proxy,
    Escape,
    Unescape,
    ArrayBuffer,
    Int8Array,
    Uint8Array,
    Uint8ClampedArray,
    Int16Array,
    Uint16Array,
    Int32Array,
    Uint32Array,
    Float32Array,
    Float64Array,
    /// `eval` reached as a *value* rather than by a direct call: `(0, eval)(src)`,
    /// `globalThis.eval(src)`, or a reference stashed and called later. Every one of those is an
    /// indirect eval, which runs in global scope; a direct call is `DirectEval`
    /// instead, because only the compiler knows what a caller's scope contains.
    Eval,
    /// Explicit resource management's two names. Appended, like everything after `Json`: the
    /// discriminant is hashed into the artifact cache key, so inserting one in the middle would
    /// invalidate every cached native build.
    DisposableStack,
    SuppressedError,
    /// The asynchronous half of the same pair. Appended for the reason above: the discriminant
    /// is part of the artifact cache key, so a new name goes at the end.
    AsyncDisposableStack,
    /// The error kind nothing throws any more. Appended for the same reason.
    EvalError,
    /// The shared-memory namespace. Appended for the same reason.
    Atomics,
    /// WHATWG UTF-8 decoder. Appended so existing artifact discriminants remain stable.
    TextDecoder,
    /// Node's check-phase task primitives. Appended to preserve existing artifact discriminants.
    SetImmediate,
    ClearImmediate,
    /// The error kind that reports several failures as one, and what `Promise.any` rejects with.
    /// Appended for the reason every name after `Json` is: the discriminant is hashed into the
    /// artifact cache key, so inserting one in the middle would invalidate every cached build.
    AggregateError,
    /// Cancellation, which every modern asynchronous API takes as an argument. Appended for the
    /// reason above.
    AbortController,
    AbortSignal,
    /// The UTF-8 encoder half of the pair whose decoder is already here.
    TextEncoder,
    StructuredClone,
}

macro_rules! builtin_bindings {
    ($( $variant:ident => $name:literal ),+ $(,)?) => {
        /// The canonical global spelling for every compiler-provided builtin.
        pub const GLOBAL_BINDINGS: &'static [(Self, &'static str)] = &[
            $( (Self::$variant, $name), )+
        ];

        /// The global name that resolves to this built-in.
        pub fn from_name(name: &str) -> Option<Self> {
            match name {
                $( $name => Some(Self::$variant), )+
                "global" => Some(Self::GlobalThis),
                _ => None,
            }
        }

        pub fn name(self) -> &'static str {
            match self {
                $( Self::$variant => $name, )+
            }
        }
    };
}

impl GlobalBuiltin {
    builtin_bindings! {
        Error => "Error",
        Symbol => "Symbol",
        String => "String",
        GlobalThis => "globalThis",
        Set => "Set",
        Map => "Map",
        Array => "Array",
        Object => "Object",
        Json => "JSON",
        Math => "Math",
        Number => "Number",
        Boolean => "Boolean",
        NotANumber => "NaN",
        Infinity => "Infinity",
        ParseInt => "parseInt",
        EncodeUri => "encodeURI",
        EncodeUriComponent => "encodeURIComponent",
        DecodeUri => "decodeURI",
        DecodeUriComponent => "decodeURIComponent",
        ParseFloat => "parseFloat",
        IsNaN => "isNaN",
        IsFinite => "isFinite",
        TypeError => "TypeError",
        RangeError => "RangeError",
        ReferenceError => "ReferenceError",
        SyntaxError => "SyntaxError",
        UriError => "URIError",
        SetTimeout => "setTimeout",
        SetInterval => "setInterval",
        ClearTimeout => "clearTimeout",
        ClearInterval => "clearInterval",
        QueueMicrotask => "queueMicrotask",
        Promise => "Promise",
        RegExp => "RegExp",
        Date => "Date",
        WeakMap => "WeakMap",
        WeakSet => "WeakSet",
        Performance => "performance",
        Reflect => "Reflect",
        BigInt => "BigInt",
        Process => "process",
        Buffer => "Buffer",
        Url => "URL",
        Function => "Function",
        Proxy => "Proxy",
        Escape => "escape",
        Unescape => "unescape",
        ArrayBuffer => "ArrayBuffer",
        Int8Array => "Int8Array",
        Uint8Array => "Uint8Array",
        Uint8ClampedArray => "Uint8ClampedArray",
        Int16Array => "Int16Array",
        Uint16Array => "Uint16Array",
        Int32Array => "Int32Array",
        Uint32Array => "Uint32Array",
        Float32Array => "Float32Array",
        Float64Array => "Float64Array",
        Eval => "eval",
        DisposableStack => "DisposableStack",
        SuppressedError => "SuppressedError",
        AsyncDisposableStack => "AsyncDisposableStack",
        EvalError => "EvalError",
        Atomics => "Atomics",
        TextDecoder => "TextDecoder",
        SetImmediate => "setImmediate",
        ClearImmediate => "clearImmediate",
        AggregateError => "AggregateError",
        AbortController => "AbortController",
        AbortSignal => "AbortSignal",
        TextEncoder => "TextEncoder",
        StructuredClone => "structuredClone",
    }
}
