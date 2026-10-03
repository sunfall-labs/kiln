use super::*;

/// `flat`: splice nested arrays into the result, down to `depth` levels.
pub(super) fn flatten(items: &[Value], depth: usize) -> Vec<Value> {
    let mut out = Vec::new();
    for item in items {
        match item {
            // A hole vanishes rather than becoming undefined, which is the one place `flat`
            // differs from copying.
            Value::Native(Native::Hole, _) => {}
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
pub(super) fn dense_items(items: &[Value]) -> Vec<Value> {
    items
        .iter()
        .map(|item| {
            if matches!(item, Value::Native(Native::Hole, _)) {
                Value::Undefined
            } else {
                item.clone()
            }
        })
        .collect()
}

/// How a console reports a run of holes: `<1 empty item>` or `<3 empty items>`.
pub(super) fn empty_items(count: usize) -> String {
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
/// The `NativeFunction` form: what `Function.prototype.toString` must answer for a built-in.
///
/// The specification allows an implementation-defined string here but requires this *shape*, and
/// the shape is what programs read. Feature detection across the ecosystem is written as
/// `/\{\s*\[native code\]/.test(String(fn))` to tell a real built-in from a polyfill that
/// replaced it, so this is not decoration -- it is the answer to a question real code asks.
pub(super) fn native_code_source(name: &str) -> String {
    // A symbol-keyed built-in carries the reserved key internally -- `get @@species` -- and that
    // is not a PropertyName any source could write, so the grammar rejects the whole string and
    // `assertNativeFunction` reports the *function* as not native. Spelled as the computed key a
    // program would actually write, which is node's answer too:
    // `function get [Symbol.species]() { [native code] }`.
    let name = match name.find(symbol::RESERVED_KEY_PREFIX) {
        Some(at) => format!(
            "{}[Symbol.{}]",
            &name[..at],
            &name[at + symbol::RESERVED_KEY_PREFIX.len()..]
        ),
        None => name.to_owned(),
    };
    format!("function {name}() {{ [native code] }}")
}

pub(super) fn native_js_name(native: &Native) -> String {
    if let Native::WebStub(kind) = native {
        return kind.name().to_owned();
    }
    if let Some(name) = crypto::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = zlib::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = os::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = path::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = querystring::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = url::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = string_decoder::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = util::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = diagnostics_channel::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = assert::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = tty::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = dns::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = perf_hooks::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = events::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = stream::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = net::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = http::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = fs::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = async_hooks::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = buffer::js_name(*native) {
        return name.to_owned();
    }
    if let Some(name) = console::js_name(*native) {
        return name.to_owned();
    }
    match native {
        Native::ProcessStartRaw => "__kilnHostProcessStart".to_owned(),
        Native::ProcessCancelRaw => "__kilnHostProcessCancel".to_owned(),
        Native::ProcessExitCodeGet => "processExitCodeGet".to_owned(),
        Native::ProcessExitCodeSet => "processExitCodeSet".to_owned(),
        Native::GcRequest => "gcRequest".to_owned(),
        Native::UrlParseRaw => "__kilnUrlParse".to_owned(),
        Native::UrlSetRaw => "__kilnUrlSet".to_owned(),
        Native::UrlFastRaw => "__kilnUrlFast".to_owned(),
        Native::UrlAdoptRaw => "__kilnUrlAdopt".to_owned(),
        Native::ArrayBufferDetachRaw => "__kilnArrayBufferDetach".to_owned(),
        Native::ArrayBufferByteLengthRaw => "__kilnArrayBufferByteLength".to_owned(),
        Native::ArrayBufferResizeRaw => "__kilnArrayBufferResize".to_owned(),
        Native::TypedArrayOutOfBoundsRaw => "__kilnTypedArrayOutOfBounds".to_owned(),
        Native::CollectionSizeRaw => "__kilnCollectionSize".to_owned(),
        Native::CloneDataRaw => "__kilnCloneData".to_owned(),
        Native::ShareRaw => "__kilnShare".to_owned(),
        Native::SharedIsRaw => "__kilnSharedIs".to_owned(),
        Native::TypedArrayCopyRaw => "__kilnTypedArrayCopy".to_owned(),
        Native::TypedArraySortRaw => "__kilnTypedArraySort".to_owned(),
        Native::DataViewStateRaw => "__kilnDataViewState".to_owned(),
        Native::IsConstructorRaw => "__kilnIsConstructor".to_owned(),
        Native::JobsPendingRaw => "__kilnJobsPending".to_owned(),
        Native::QueueIoRaw => "__kilnQueueIo".to_owned(),
        Native::TimerRefRaw => "__kilnTimerRef".to_owned(),
        Native::WorkerOpRaw => "__kilnWorkerOp".to_owned(),
        Native::PortWaitRaw => "__kilnPortWait".to_owned(),
        Native::PromiseRejectionsRaw => "__kilnPromiseRejections".to_owned(),
        Native::RegExpLegacyCaptureRaw => "__kilnRegExpLegacyCapture".to_owned(),
        Native::NodeModuleCreateRequire => "__kilnHostModuleCreateRequire".to_owned(),
        // A symbol-keyed method is named for the symbol in brackets, which is what the
        // specification's `SetFunctionName` does with a symbol key. Derivation would spell it
        // `functionHasInstance`, the name of a Rust variant rather than of anything a program
        // can see.
        Native::FunctionHasInstance => "[Symbol.hasInstance]".to_owned(),
        Native::SymbolToPrimitive => "[Symbol.toPrimitive]".to_owned(),
        // An accessor is named for what it reads, prefixed with the half it is.
        Native::TypedArrayToStringTag => "get [Symbol.toStringTag]".to_owned(),
        Native::StringToStringMethod => "toString".to_owned(),
        Native::StringValueOf | Native::SymbolValueOf | Native::BigIntValueOf => {
            "valueOf".to_owned()
        }
        // A `DataView` accessor is named for the member it is published under, not for the Rust
        // variant behind it. Sixteen of them share four variants, so derivation gave all eight
        // getters the name `dataViewGet` -- which test262 checks by descriptor, and which any
        // program printing a stack or a method table shows.
        Native::DataViewGet(kind) => format!("get{}", kind.element_name()),
        Native::DataViewSet(kind) => format!("set{}", kind.element_name()),
        Native::DataViewGetBig { signed: true } => "getBigInt64".to_owned(),
        Native::DataViewGetBig { signed: false } => "getBigUint64".to_owned(),
        Native::DataViewSetBig { signed: true } => "setBigInt64".to_owned(),
        Native::DataViewSetBig { signed: false } => "setBigUint64".to_owned(),
        // `Array.prototype.toString` is `join` with no separator, which is what the variant is
        // named after -- but the member is `toString`.
        Native::ArrayJoinDefault => "toString".to_owned(),
        // Derivation cannot reach these: the payload is the whole of the name.
        Native::Intl(op) => intl::js_name(*op),
        Native::Temporal(op) => temporal::js_name(*op),
        Native::Error(kind) => kind.name().to_owned(),
        Native::Date(method) => method.js_name().to_owned(),
        // An accessor is named for what it reads, prefixed with the half it is -- which is what
        // a stack trace through `Object.getOwnPropertyDescriptor(...).get` prints.
        Native::RegExpAccessor(accessor) => format!("get {}", accessor.property_key()),
        Native::BuiltinAccessor(accessor) => format!("get {}", accessor.property_key()),
        Native::SetOperation(operation) => operation.property_key().to_owned(),
        Native::Uint8Text(conversion) => conversion.key().to_owned(),
        Native::ArrayCopy(method) => method.property_key().to_owned(),
        Native::RegExpSymbol(method) => method.js_name(),
        // The derivation works from the variant's `Debug` spelling, and for these the payload is
        // the whole of the name: `IteratorLazy(Map)` would otherwise be `iteratorLazy`.
        Native::IteratorLazy(kind) => kind.property_key().to_owned(),
        Native::IteratorTerminal(kind) => kind.property_key().to_owned(),
        // The derivation has no `Iterator` in its table of type prefixes, so these three would
        // keep it. Only the first is a name a program can read; the other two are the helper's
        // own methods, named for what they are.
        // The derivation strips a leading type name, which for these three takes a word that is
        // part of the method rather than part of the type: `WeakRefDeref` lowers to `refDeref`.
        Native::AtomicsOp(op) => op.property_key().to_owned(),
        Native::CollectionGetOrInsert {
            computed: false, ..
        } => "getOrInsert".to_owned(),
        Native::CollectionGetOrInsert { computed: true, .. } => "getOrInsertComputed".to_owned(),
        Native::WeakRefDeref => "deref".to_owned(),
        Native::DisposableStackUse => "use".to_owned(),
        Native::DisposableStackAdopt => "adopt".to_owned(),
        Native::DisposableStackDefer => "defer".to_owned(),
        Native::DisposableStackMove => "move".to_owned(),
        Native::DisposableStackDispose => "dispose".to_owned(),
        Native::AsyncDisposableStackUse => "use".to_owned(),
        Native::AsyncDisposableStackAdopt => "adopt".to_owned(),
        Native::AsyncDisposableStackDefer => "defer".to_owned(),
        Native::AsyncDisposableStackMove => "move".to_owned(),
        Native::AsyncDisposableStackDisposeAsync => "disposeAsync".to_owned(),
        // The derivation lowers a trailing acronym as an ordinary word: `JsonRawJson` becomes
        // `rawJson`, and the language spells the second half in capitals.
        Native::JsonRawJson => "rawJSON".to_owned(),
        Native::JsonIsRawJson => "isRawJSON".to_owned(),
        Native::FinalizationRegistryRegister => "register".to_owned(),
        Native::FinalizationRegistryUnregister => "unregister".to_owned(),
        // The derivation has no `Iterator` in its table of type prefixes, so it spells this
        // `iteratorFrom` rather than `from`.
        Native::IteratorFrom => "from".to_owned(),
        Native::ShadowRealmConstructor => "ShadowRealm".to_owned(),
        Native::ShadowRealmEvaluate => "evaluate".to_owned(),
        Native::ShadowRealmImportValue => "importValue".to_owned(),
        Native::ThrowTypeError => String::new(),
        Native::ProtoGetter => "get __proto__".to_owned(),
        Native::ProtoSetter => "set __proto__".to_owned(),
        // The derivation strips the type and lowers what is left, which spells this `utc`. A
        // case distinction the variant does not carry cannot be recovered from it.
        Native::DateUtc => "UTC".to_owned(),
        // A view constructor is named for its element type -- `Uint8Array`, `BigInt64Array` --
        // and the derivation cannot reach it: the payload it discards as "not part of the name"
        // is the only place the type is written, so what is left spells the family instead.
        Native::TypedArrayConstructor(kind) => kind.name().to_owned(),
        // The URI four are an acronym the variant spelling flattens: `DecodeUri` lowers to
        // `decodeUri`, and the language spells it `decodeURI`. A derivation cannot recover a case
        // distinction that the input does not carry, so these are named outright.
        // `SetTimeout` starts with a type name that is not its type: the derivation strips `Set`
        // -- the collection -- and answers `timeout`. A prefix that is a word in its own right is
        // the one thing a name derived from a variant cannot get right on its own.
        // THE SHARED SURFACE'S OWN NAMES.
        //
        // `kiln_builtin_surface::SURFACE` says which members each built-in has, and the key it
        // says it under *is* the name that member reports -- node makes that true for every
        // static on every built-in, measured. The native backend now reads the name out of the
        // row; this derivation cannot, because the table is keyed by `(global, member)` and
        // nothing here maps a `Native` back to one without inverting the crate dependency.
        //
        // So these are spelled out, and `every_surface_member_is_named_for_its_key` holds them to
        // the table by reading each member through the real property path. Thirteen of them were
        // wrong when that guard was written: the derivation strips a known type prefix and
        // lowercases what is left, and it has no prefix for `NodeBuffer`, `AbortSignal`,
        // `Performance` or `Error` -- so `Buffer.concat.name` was `"nodeBufferConcat"`, which is
        // this file's spelling of a variant handed to a program that asked what a function is
        // called. `ObjectOwnNames` is the other shape: a variant named for neither its type nor
        // its member, which no prefix rule could have recovered.
        //
        // Adding a prefix to the derivation's table would have fixed some of these and is the
        // wrong move: a longest-prefix strip over a growing list is what produced them, and
        // "Error" as a prefix would rename every error variant that follows it.
        Native::ObjectOwnNames => "getOwnPropertyNames".to_owned(),
        Native::ErrorCaptureStackTrace => "captureStackTrace".to_owned(),
        Native::ErrorIsError => "isError".to_owned(),
        // The derivation lowercases only the first character, so a name whose own spelling is
        // lower-case past a digit comes back with a capital in it: `f16Round` for `f16round`.
        Native::MathF16Round => "f16round".to_owned(),
        Native::PerformanceNow => "now".to_owned(),
        Native::AbortSignalAbort => "abort".to_owned(),
        Native::AbortSignalTimeout => "timeout".to_owned(),
        Native::AbortSignalAny => "any".to_owned(),
        // FOUR NAMES THE DERIVATION SPELLED OUT OF THE RUST VARIANT.
        //
        // These are not near-misses of capitalisation: each one handed a program the identifier
        // this implementation happens to use. `Buffer.name` read "nodeBufferGlobal" and `URL.name`
        // read "Url" -- node answers "Buffer" and "URL" -- and the two `node:` modules whose
        // default export is callable answered "nodeAssertOk" and "nodeEventEmitter" where node
        // answers "assert" and "EventEmitter". A name is what a program reads to tell one
        // function from another, so an internal spelling in it is a wrong answer and not a
        // cosmetic one; the native backend already gave three of the four the node answer, so
        // this was also the two engines disagreeing.
        Native::NodeEventEmitter => "EventEmitter".to_owned(),
        Native::SetTimeout => "setTimeout".to_owned(),
        Native::SetInterval => "setInterval".to_owned(),
        Native::SetImmediate => "setImmediate".to_owned(),
        Native::ClearImmediate => "clearImmediate".to_owned(),
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
pub(super) const fn namespace_name(native: &Native) -> Option<&'static str> {
    match native {
        Native::Math => Some("Math"),
        Native::Json => Some("JSON"),
        Native::Intl(op) => intl::namespace_name(*op),
        Native::Temporal(op) => temporal::namespace_name(*op),
        Native::Reflect => Some("Reflect"),
        Native::Atomics => Some("Atomics"),
        // Lower-case because that is what the language calls it: `performance` is an object a
        // program reads `now` from, not a constructor it names. Being here rather than in the
        // callable arm is what makes `String(performance)` an object tag and stops
        // `Object.getPrototypeOf(performance)` claiming `Function.prototype` -- the same two
        // consequences `typeof` has, reaching the other two conversions.
        Native::Performance => Some("performance"),
        _ => None,
    }
}

pub(super) fn builtin_js_name(variant: &str) -> String {
    // A payload is not part of the name: `TypedArrayFrom(Uint8)` is `Uint8Array.from`, not a
    // function called `TypedArrayFrom(Uint8)`.
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
    const TYPES: [&str; 29] = [
        "AsyncGenerator",
        // Before the bare `Weak`, which is what the ordering rule above is for: `WeakMapGet`
        // lost only its first word and came out as `mapGet`, so every weak collection method
        // was named after a Rust variant rather than after itself.
        "WeakMap",
        "WeakSet",
        "ArrayBuffer",
        "TypedArray",
        "Collection",
        "Generator",
        "Primitive",
        // `Function.prototype.toString` is called `toString`, not `functionToString` -- and it is
        // the one whose name a program is most likely to read, because it is the one whose own
        // answer now names it. `FunctionConstructor` is not caught here: the suffix rule above
        // has already turned it into `Function`.
        "Function",
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

pub(super) fn inspect_key(key: &str) -> String {
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
pub(super) fn quote_for_console(text: &str) -> String {
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
pub(super) struct InspectGuard<'a> {
    pub(super) seen: &'a mut Vec<usize>,
}

impl<'a> InspectGuard<'a> {
    pub(super) fn enter(seen: &'a mut Vec<usize>, container: *const ()) -> Option<Self> {
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
#[cfg(test)]
pub(super) fn index_key(key: &str) -> Option<u32> {
    let index: u32 = key.parse().ok()?;
    (index != u32::MAX && index.to_string() == key).then_some(index)
}

#[deny(clippy::wildcard_enum_match_arm)]
#[cfg(test)]
pub(super) fn loose_equal(left: &Value, right: &Value) -> bool {
    loose_equal_checked(left, right).expect("test string coercion")
}

/// `ToPrimitive` with no hint, which for everything here means `toString`.
///
/// `valueOf` comes first in the specification and is skipped because nothing yet returns a
/// primitive from it; when a user-defined `valueOf` can exist, this needs the interpreter and
/// moves onto it.
pub(super) fn to_primitive(value: &Value) -> Result<Value, ExecutionError> {
    Ok(Value::String(value.to_string_units_checked()?))
}

pub(super) fn compare(left: &Value, right: &Value, predicate: OrderingPredicate) -> bool {
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
pub(super) enum OrderingPredicate {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

pub(super) fn parse_number(value: &str) -> f64 {
    kiln_host_abi::string_to_number(value)
}

pub(super) fn parse_number_units(value: &StringUnits) -> f64 {
    value
        .to_scalar_string()
        .map_or(f64::NAN, |text| parse_number(&text))
}

/// A number as `String(value)` renders it.
///
/// Delegates to the ABI crate rather than repeating the rules: both halves must agree about what
/// `String(1e21)` is, and the surest way for two copies to agree is for there to be one.
pub(crate) fn number_string(value: f64) -> String {
    kiln_host_abi::number_to_js_string(value)
}
