use super::*;

/// The plain-data subset of the structured clone, for `__kilnCloneData`: an ordinary object with
/// the default prototype whose own properties are all enumerable data properties under string
/// keys, an array with no named properties and no exotic elements, and primitives, which are
/// shared. `None` for anything else -- including a reserved `@@` key (a symbol or the machine's
/// own bookkeeping, such as a private field) -- so the caller falls back to the general clone.
/// Nothing read here can run program code, so giving up half way has no effect to undo.
pub(super) fn clone_plain_data(
    value: &Value,
    seen: &mut std::collections::HashMap<usize, Value>,
    depth: usize,
) -> Option<Value> {
    const MAX_DEPTH: usize = 512;
    match value {
        Value::Undefined
        | Value::Null
        | Value::Boolean(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::BigInt(_) => Some(value.clone()),
        Value::Object(object) => {
            let identity = Rc::as_ptr(object) as *const () as usize;
            if let Some(copy) = seen.get(&identity) {
                return Some(copy.clone());
            }
            if depth > MAX_DEPTH {
                return None;
            }
            let entries: Vec<(StringUnits, Value)> = {
                let data = object.borrow();
                if data.prototype.is_some()
                    || data.error_brand
                    || data.arguments_brand
                    || !data.extensible
                    || data.frozen
                    || data.module_namespace
                    || !data.namespace_cells.is_empty()
                    || data.primitive.is_some()
                    || data.node_events.is_some()
                    || data.node_object.is_some()
                    || data.buffer_brand
                    || data.shadow_realm.is_some()
                {
                    return None;
                }
                let mut entries = Vec::with_capacity(data.properties.len());
                for slot in &data.properties {
                    let Property::Value(item) = &slot.property else {
                        return None;
                    };
                    if !slot.attributes.enumerable
                        || (slot.key.unit_at(0) == Some(b'@' as u16)
                            && slot.key.unit_at(1) == Some(b'@' as u16))
                    {
                        return None;
                    }
                    entries.push((slot.key.clone(), item.clone()));
                }
                entries
            };
            let copy = Rc::new(RefCell::new(ObjectData::default()));
            seen.insert(identity, Value::Object(Rc::clone(&copy)));
            for (key, item) in entries {
                let item = clone_plain_data(&item, seen, depth + 1)?;
                copy.borrow_mut()
                    .define_value_units(&key, item, Attributes::DEFAULT);
            }
            Some(Value::Object(copy))
        }
        Value::Array(array) => {
            let identity = Rc::as_ptr(array) as *const () as usize;
            if let Some(copy) = seen.get(&identity) {
                return Some(copy.clone());
            }
            if depth > MAX_DEPTH
                || !array.properties.borrow().properties.is_empty()
                || !array.elements.borrow().is_empty()
                || array.length.get() != array.items.borrow().len()
                || array.sparse.get()
            {
                return None;
            }
            let items = array.items.borrow().clone();
            let copy = ArrayData::new(Vec::new());
            seen.insert(identity, Value::Array(Rc::clone(&copy)));
            let mut cloned = Vec::with_capacity(items.len());
            for item in &items {
                cloned.push(match item {
                    Value::Native(Native::Hole, _) => item.clone(),
                    _ => clone_plain_data(item, seen, depth + 1)?,
                });
            }
            let holes = cloned
                .iter()
                .any(|item| matches!(item, Value::Native(Native::Hole, _)));
            copy.sparse.set(holes);
            copy.length.set(cloned.len());
            *copy.items.borrow_mut() = cloned;
            Some(Value::Array(copy))
        }
        _ => None,
    }
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
/// Convert an owned host result at the Node Adapter boundary.
pub(super) fn node_host_value(value: AsyncHostValue) -> Result<Value, ExecutionError> {
    Ok(match value {
        AsyncHostValue::Undefined => Value::Undefined,
        AsyncHostValue::String(s) => Value::String(s.into()),
        AsyncHostValue::DirEntries(entries) => {
            let mut out = Vec::new();
            for entry in entries {
                out.push(Value::String(entry.name.into()));
            }
            Value::Array(ArrayData::new(out))
        }
        AsyncHostValue::Bytes(bytes) => {
            let length = bytes.len();
            Value::TypedArray(Rc::new(TypedArrayData {
                kind: TypedKind::Uint8,
                buffer: Rc::new(BufferData {
                    bytes: Bytes::new(false, bytes),
                    max_byte_length: None,
                    shared: false,
                    detached: std::cell::Cell::new(false),
                    immutable: false,
                    properties: RefCell::new(ObjectData::default()),
                }),
                offset: 0,
                length: Some(length),
                properties: RefCell::new(ObjectData::default()),
            }))
        }
        AsyncHostValue::FileStat(stat) => {
            let mut object = ObjectData::default();
            object.set("size", Value::Number(stat.size as f64));
            object.set_non_enumerable("@@kiln:node:is-file", Value::Boolean(stat.is_file));
            object.set_non_enumerable(
                "@@kiln:node:is-directory",
                Value::Boolean(stat.is_directory),
            );
            object.set_non_enumerable("@@kiln:node:is-symlink", Value::Boolean(stat.is_symlink));
            object.set(
                "isFile",
                Value::Native(Native::NodeStatIsFile, RealmId::PRIMARY),
            );
            object.set(
                "isDirectory",
                Value::Native(Native::NodeStatIsDirectory, RealmId::PRIMARY),
            );
            object.set(
                "isSymbolicLink",
                Value::Native(Native::NodeStatIsSymbolicLink, RealmId::PRIMARY),
            );
            Value::Object(Rc::new(RefCell::new(object)))
        }
        AsyncHostValue::Resource(resource) => {
            return Err(ExecutionError::Host {
                instruction: 0,
                message: format!(
                    "persistent Node resource {} has no JavaScript representation yet",
                    resource.id.get()
                ),
            });
        }
        AsyncHostValue::AcceptedTcpStream(accepted) => {
            return Err(ExecutionError::Host {
                instruction: 0,
                message: format!(
                    "accepted Node resource {} has no JavaScript representation yet",
                    accepted.resource.id.get()
                ),
            });
        }
        AsyncHostValue::ProcessOutput(output) => {
            let mut object = ObjectData::default();
            object.set(
                "pid",
                output
                    .pid
                    .map_or(Value::Undefined, |pid| Value::Number(pid as f64)),
            );
            object.set(
                "stdout",
                node_host_value(AsyncHostValue::Bytes(output.stdout))?,
            );
            object.set(
                "stderr",
                node_host_value(AsyncHostValue::Bytes(output.stderr))?,
            );
            object.set(
                "exitCode",
                output
                    .exit_code
                    .map_or(Value::Null, |code| Value::Number(code as f64)),
            );
            object.set(
                "signal",
                output
                    .signal
                    .map_or(Value::Null, |signal| Value::Number(signal as f64)),
            );
            Value::Object(Rc::new(RefCell::new(object)))
        }
        AsyncHostValue::ResourceState(state) => {
            return Err(ExecutionError::Host {
                instruction: 0,
                message: format!(
                    "Node resource state for {} must be consumed by its typed adapter",
                    state.info.id.get()
                ),
            });
        }
    })
}

/// `{ value, done }`, the shape every `next` returns.
pub(super) fn iteration_result(value: Value, done: bool) -> Value {
    let mut result = ObjectData::default();
    result.set("value", value);
    result.set("done", Value::Boolean(done));
    Value::Object(Rc::new(RefCell::new(result)))
}

/// The trace recorded on an error object when it was constructed, if it is one.
pub(super) fn captured_trace(thrown: &Value) -> Vec<String> {
    let Value::Object(object) = thrown else {
        return Vec::new();
    };
    match object.borrow().get_own(ERROR_TRACE) {
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
pub(super) fn describe_thrown(value: &Value) -> String {
    if has_error_to_string(value) {
        error_to_string(value)
    } else {
        value.to_display_string()
    }
}

/// Whether a value inherits `Error.prototype.toString`.
pub(super) fn has_error_to_string(value: &Value) -> bool {
    match value {
        Value::Object(object) => matches!(
            object.borrow().get("toString"),
            Some(Value::Native(Native::ErrorToString, _))
        ),
        _ => false,
    }
}

/// `Error.prototype.toString`: `name`, then `: message` when the message is non-empty.
pub(super) fn error_to_string(value: &Value) -> String {
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

/// The name read from a constructor value in an object's prototype chain.
pub(super) fn constructor_name(value: &Value) -> Option<String> {
    match value {
        Value::Function(closure) => match closure.properties.borrow().get_own("name") {
            Some(Value::String(name)) => Some(name.to_lossy_string()),
            _ => None,
        },
        Value::Native(Native::Error(kind), _) => Some(kind.name().to_owned()),
        _ => None,
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
#[cfg(test)]
pub(super) fn instance_of(
    value: &Value,
    constructor: &Value,
    error_prototype: Option<&Value>,
) -> bool {
    let prototype = match constructor {
        Value::Function(closure) => match closure.prototype.borrow().clone() {
            Some(prototype) => prototype,
            // A constructor never used with `new` and never read has no prototype object yet,
            // so nothing can descend from it.
            None => return false,
        },
        // A native constructor's prototype is the realm's, not one per closure.
        Value::Native(Native::Error(_), _) => match error_prototype {
            Some(prototype) => prototype.clone(),
            None => return false,
        },
        // These have no prototype object to walk -- each is its own value kind -- so the test is
        // what the chain would have answered.
        Value::Native(Native::PromiseConstructor, _) => {
            return matches!(value, Value::Promise(_));
        }
        Value::Native(Native::DateConstructor, _) => return matches!(value, Value::Date(_)),
        // An `Intl` value is an instance of its own service and of nothing else: a `Collator`
        // is not an `Intl.NumberFormat` even though both are `Value::Intl`.
        Value::Native(Native::Intl(intl::Op::Constructor(service)), _) => {
            return matches!(value, Value::Intl(built) if built.service() == *service);
        }
        Value::Native(Native::RegExpConstructor, _) => return matches!(value, Value::RegExp(_)),
        Value::Native(Native::ArrayConstructor, _) => return matches!(value, Value::Array(_)),
        // A view is an instance of the constructor that names its element type, and of nothing
        // else: `new Uint8Array(1) instanceof Int8Array` is false.
        Value::Native(Native::TypedArrayConstructor(kind), _) => {
            return matches!(value, Value::TypedArray(view) if view.kind == *kind);
        }
        // The two buffer types are told apart by their flag and by nothing else: a shared
        // buffer is not an `ArrayBuffer`, which is what makes `ArrayBuffer.isView` and every
        // brand check in the specification able to refuse it.
        Value::Native(Native::ArrayBufferConstructor, _) => {
            return matches!(value, Value::ArrayBuffer(buffer) if !buffer.shared);
        }
        Value::Native(Native::SharedArrayBufferConstructor, _) => {
            return matches!(value, Value::ArrayBuffer(buffer) if buffer.shared);
        }
        Value::Native(Native::DataViewConstructor, _) => {
            return matches!(value, Value::DataView(_));
        }
        // Every iterator this realm makes inherits `%IteratorPrototype%`, and `Iterator` is what
        // that prototype belongs to. The marker is the same one method lookup uses, so a value
        // cannot answer `map` and then deny being an iterator. A guard rather than an arm: a
        // value that carries no marker still gets the ordinary chain walk below, which is what
        // answers for an iterator a program wrote by extending `Iterator` itself.
        Value::Native(Native::IteratorConstructor, _)
            if match value {
                Value::Generator(_) => true,
                Value::Object(object) => {
                    let object = object.borrow();
                    object.get_own("@@iterated").is_some()
                        || object.get_own(HELPER_SOURCE).is_some()
                }
                _ => false,
            } =>
        {
            return true;
        }
        // A Buffer is a branded view, and `Buffer.prototype` -- when the bootstrap has assigned
        // one -- is what the chain reaches; a `Uint8Array` without the brand is not one.
        Value::Native(Native::NodeBufferGlobal, _) => match buffer_prototype() {
            Some(prototype) => prototype,
            None => return matches!(value, Value::TypedArray(view) if is_buffer_view(view)),
        },
        // A Set or Map is an object carrying reserved state, so the test is for that state
        // rather than for a prototype the realm does not yet have as an object.
        Value::Native(kind @ (Native::SetConstructor | Native::MapConstructor), _) => {
            let Value::Object(object) = value else {
                return false;
            };
            let object = object.borrow();
            return object.get_own("@@entries").is_some()
                && object.get_own("@@map").is_some() == matches!(kind, Native::MapConstructor);
        }
        // Any other built-in that has a `prototype`: the chain is walked against it, which is
        // what makes `Object instanceof Object` true -- `Object` is a function, so its chain
        // reaches `Function.prototype` and then `Object.prototype`, which is what it is being
        // compared against. A plain-object check asks exactly that.
        Value::Native(native, _) => match native.property("prototype") {
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
/// The realm a value belongs to: `GetFunctionRealm` for a callable, the marker's own realm for
/// an intrinsic, and the primary realm for everything else.
///
/// Only the two `Copy` markers and a closure carry a realm of their own. Every other value
/// expresses the realm it was made in through its `[[Prototype]]`, which is what the
/// specification says the difference *is*: an array built in realm 1 links to realm 1's
/// `Array.prototype` and is otherwise an ordinary array.
pub(super) fn value_realm(value: &Value) -> RealmId {
    match value {
        Value::Native(_, realm) | Value::Callback(_, realm) => *realm,
        Value::Function(closure) => closure.realm,
        // A bound function and a proxy both answer for what they wrap, which is what
        // `GetFunctionRealm` says: neither has a realm of its own to report.
        Value::Bound(bound) => value_realm(&bound.target),
        // A wrapped function does *not*, and this is the line that keeps the boundary honest.
        // It has a realm of its own -- the caller's -- and answering with its target's would
        // send every `TypeError` it raises back into the realm the caller must not see, and
        // hand it that realm's `Error.prototype` to inspect.
        Value::Wrapped(wrapped) => wrapped.realm,
        Value::Proxy(proxy) => value_realm(&proxy.target),
        Value::Shared(facade) => facade.realm,
        _ => RealmId::PRIMARY,
    }
}

/// Rewrite an intrinsic minted by a table so that it names `realm`'s copy of that built-in.
///
/// The property tables are pure and realm-blind by design -- `Native::property` cannot be handed
/// a receiver -- so they mint every intrinsic in the primary realm and the caller retags what
/// comes back from the receiver it asked. Anything that is not a marker is returned untouched,
/// because it is not an intrinsic: a value a program *stored* on a built-in keeps whatever realm
/// it already had.
pub(super) fn in_realm(value: Value, realm: RealmId) -> Value {
    if realm == RealmId::PRIMARY {
        return value;
    }
    match value {
        Value::Native(native, RealmId::PRIMARY) => Value::Native(native, realm),
        Value::Callback(method, RealmId::PRIMARY) => Value::Callback(method, realm),
        other => other,
    }
}

thread_local! {
    /// `Buffer.prototype`: the object the shared bootstrap (`kiln-builtins/src/buffer.ts`)
    /// assigns to the `Buffer` built-in's store. Held here as well because the two places that
    /// need it -- a branded view's member read and `Object.getPrototypeOf` of one -- are free
    /// functions of the value, with no store in hand. Written by `Intrinsics::install_key`.
    pub(super) static BUFFER_PROTOTYPE: RefCell<Option<Value>> = const { RefCell::new(None) };
}

/// The current `Buffer.prototype`, if a bootstrap has assigned one.
pub(super) fn buffer_prototype() -> Option<Value> {
    BUFFER_PROTOTYPE.with(|slot| slot.borrow().clone())
}

/// An own property of `Buffer.prototype` by name, for a branded view's read.
pub(super) fn buffer_prototype_own(name: &str) -> Option<Value> {
    match buffer_prototype()? {
        Value::Object(object) => object.borrow().get_own(name).cloned(),
        _ => None,
    }
}

/// Whether a view carries the Buffer brand.
pub(super) fn is_buffer_view(view: &TypedArrayData) -> bool {
    view.properties.borrow().buffer_brand
}

pub(super) fn prototype_of(value: &Value) -> Value {
    if let Value::Shared(facade) = value {
        return facade.prototype();
    }
    let realm = value_realm(value);
    // A subclassed value keeps its prototype with its other non-element properties, which is what
    // makes `new (class extends Array {})() instanceof that class` true -- and, through the same
    // store, what makes it true of a view, a buffer, a regexp and a bound function as well.
    let recorded = prototype_slot(value).and_then(|slot| slot.borrow().prototype().cloned());
    if let Some(link) = recorded {
        return link;
    }
    match value {
        // `Object.prototype` is the end of every chain; the other tables inherit from it.
        Value::Native(Native::PrototypeOf(PrototypeTable::Object), _) => Value::Null,
        // The one two-level chain among the tables: `Int8Array.prototype` inherits from
        // `%TypedArray%.prototype`, which is where the shared methods live and is why
        // `Int8Array.prototype.map` finds one at all.
        Value::Native(Native::PrototypeOf(PrototypeTable::TypedArrayFor(_)), _) => {
            Value::Native(Native::PrototypeOf(PrototypeTable::TypedArray), realm)
        }
        // The five built-in iterator prototypes and the two generator ones all sit on
        // `%IteratorPrototype%`, which is where the helpers live and what `instanceof Iterator`
        // walks to. Falling to `Object.prototype` with the arm below broke exactly that:
        // `Iterator.from(x)` must hand back `x` when it is already an iterator, and stopped.
        Value::Native(
            Native::AsyncFunctionConstructor
                | Native::AsyncGeneratorFunctionConstructor
                | Native::GeneratorFunctionConstructor,
            _,
        ) => Value::Native(Native::FunctionConstructor, realm),
        Value::Native(
            Native::AsyncFunctionPrototype
                | Native::AsyncGeneratorFunctionPrototype
                | Native::GeneratorFunctionPrototype,
            _,
        ) => Value::Native(Native::PrototypeOf(PrototypeTable::Function), realm),
        Value::Native(Native::PrototypeOf(PrototypeTable::AsyncGenerator), _) => {
            Value::Native(Native::PrototypeOf(PrototypeTable::AsyncIterator), realm)
        }
        Value::Native(Native::PrototypeOf(PrototypeTable::AsyncIterator), _) => {
            Value::Native(Native::PrototypeOf(PrototypeTable::Object), realm)
        }
        Value::Native(
            Native::PrototypeOf(
                PrototypeTable::IteratorFor(_)
                | PrototypeTable::Generator,
            ),
            _,
        ) => Value::Native(Native::PrototypeOf(PrototypeTable::Iterator), realm),
        Value::Native(Native::PrototypeOf(_), _) => {
            Value::Native(Native::PrototypeOf(PrototypeTable::Object), realm)
        }
        // Each `Intl` instance links to its own service's prototype, and each of those to
        // `Object.prototype`. Without the first link `Object.getPrototypeOf(new Intl.Collator())`
        // is null and every method on it is found only because the value kind answers directly --
        // which is a chain a program can see is missing.
        Value::Intl(built) => {
            Value::Native(Native::Intl(intl::Op::Prototype(built.service())), realm)
        }
        Value::Native(Native::Intl(intl::Op::Prototype(_) | intl::Op::Namespace), _) => {
            Value::Native(Native::PrototypeOf(PrototypeTable::Object), realm)
        }
        // Each of the eight `Temporal` types links to its own prototype, for exactly the reason
        // an `Intl` formatter links to its service's. Without it the chain a `Temporal` value
        // reports is empty, and `d instanceof Temporal.PlainDate` is false for a `d` the program
        // built one line earlier -- a wrong answer rather than a missing one, because
        // `d.constructor === Temporal.PlainDate` was already true and the brand tag already read
        // `[object Temporal.PlainDate]`. The methods were found regardless because the value kind
        // answers them itself, which is why the missing link stayed invisible until something
        // walked the chain.
        Value::Temporal(built) => Value::Native(
            Native::Temporal(temporal::Op::Prototype(built.kind())),
            realm,
        ),
        // And each of those eight prototypes is an ordinary object inheriting `Object.prototype`.
        // It is not a callable, so it must not take the arm below and claim `Function.prototype`:
        // `Object.getPrototypeOf(Temporal.PlainDate.prototype)` is what a program checks, and
        // `Temporal.PlainDate.prototype instanceof Function` must be false.
        Value::Native(Native::Temporal(temporal::Op::Prototype(_)), _) => {
            Value::Native(Native::PrototypeOf(PrototypeTable::Object), realm)
        }
        // The nine view constructors inherit from `%TypedArray%` rather than straight from
        // `Function.prototype`, which is the only way a program can name the abstract
        // constructor: `Object.getPrototypeOf(Int8Array)` is what every use of it starts from.
        Value::Native(Native::TypedArrayConstructor(_), _) => {
            Value::Native(Native::TypedArrayIntrinsic, realm)
        }
        // NativeError constructors inherit static properties from Error, just as the built-in
        // constructors do in ECMAScript. This is also what makes `TypeError.isError` resolve as an
        // inherited method rather than an accidental own property.
        Value::Native(Native::Error(kind), _) if *kind != ErrorKind::Error => {
            Value::Native(Native::Error(ErrorKind::Error), realm)
        }
        // `Math`, `JSON`, `Reflect` and `Atomics` are ordinary objects, not functions: none of
        // them is callable, and `Object.getPrototypeOf(Math)` is `Object.prototype`. Spelled as
        // natives here, they were taking the callable arm below and claiming `Function.prototype`
        // -- which made `Math instanceof Function` true and put `call` and `apply` on a value
        // that has no call behaviour to bind.
        Value::Native(native, _) if namespace_name(native).is_some() => {
            Value::Native(Native::PrototypeOf(PrototypeTable::Object), realm)
        }
        // Every callable inherits `Function.prototype`, and every ordinary object
        // `Object.prototype`. A primitive has no chain of its own to walk. The two the promise
        // machinery makes are callables too -- a program is handed `resolve` and `reject` and can
        // ask what they inherit from, and answering null said they were not functions at all.
        Value::Function(closure) => {
            function_kind_prototype(closure).map_or_else(
                || Value::Native(Native::PrototypeOf(PrototypeTable::Function), realm),
                |prototype| Value::Native(prototype, realm),
            )
        }
        Value::Native(_, _)
        | Value::Bound(_)
        // A wrapped function's `realm` is its own -- the caller's -- so this reads
        // `Function.prototype` *of the realm that made the wrapper*, which is precisely what
        // `WrappedFunctionCreate` sets and what a program checks with `getPrototypeOf`.
        | Value::Wrapped(_)
        | Value::Callback(_, _)
        | Value::Settler(..)
        | Value::Capability(_) | Value::Element(_) => {
            Value::Native(Native::PrototypeOf(PrototypeTable::Function), realm)
        }
        Value::Array(_) => Value::Native(Native::PrototypeOf(PrototypeTable::Array), realm),
        // A date is a value kind rather than an object here, and without this link
        // `Date.prototype.isPrototypeOf(new Date())` is false -- a chain a program can see is
        // missing, and the one every `instanceof Date` walks.
        Value::Date(_) => Value::Native(Native::PrototypeOf(PrototypeTable::Date), realm),
        Value::Promise(_) => Value::Native(Native::PrototypeOf(PrototypeTable::Promise), realm),
        // A generator inherits from its *function's* `prototype`, exactly as an instance does --
        // and that object now points at `%GeneratorPrototype%`. This answered null, so every walk
        // of a generator's chain ended before it reached a method.
        Value::Generator(state) => prototype_object(&state.borrow().closure),
        // A regexp is a value kind here rather than an object, and without this link
        // `/x/ instanceof RegExp` is false and `RegExp.prototype.isPrototypeOf(/x/)` denies it --
        // a chain a program can see is missing, and the one every brand check walks.
        Value::RegExp(_) => Value::Native(Native::PrototypeOf(PrototypeTable::RegExp), realm),
        Value::Object(_) => Value::Native(Native::PrototypeOf(PrototypeTable::Object), realm),
        Value::ArrayBuffer(buffer) => Value::Native(
            Native::PrototypeOf(if buffer.shared {
                PrototypeTable::SharedArrayBuffer
            } else {
                PrototypeTable::ArrayBuffer
            }),
            realm,
        ),
        Value::DataView(_) => Value::Native(Native::PrototypeOf(PrototypeTable::DataView), realm),
        Value::TypedArray(view) if is_buffer_view(view) && buffer_prototype().is_some() => {
            buffer_prototype().unwrap_or(Value::Null)
        }
        Value::TypedArray(view) => Value::Native(
            Native::PrototypeOf(PrototypeTable::TypedArrayFor(view.kind)),
            realm,
        ),
        _ => Value::Null,
    }
}

/// Whether `Type(value)` is Object, in the specification's sense.
///
/// The reflection statics all begin by asking it: `Object.isFrozen`, `Object.isSealed` and
/// `Object.isExtensible` each answer for a primitive by definition and have to *look* for an
/// object. Spelled once rather than as a `_` arm in each, because the two that had one disagreed
/// -- `Object.isFrozen(Object)` was `true` while `Object.isSealed(Object)` was `false`, which no
/// object can be at once.
pub(super) fn is_object_type(value: &Value) -> bool {
    !matches!(
        value,
        Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::BigInt(_)
            | Value::Symbol(_)
            | Value::Cell(_)
    )
}

/// `key in target`.
pub(super) fn has_property_checked(
    intrinsics: &Intrinsics,
    target: &Value,
    key: &Value,
) -> Result<bool, ExecutionError> {
    let units = key.property_key_units_checked()?;
    Ok(has_property_key(
        intrinsics,
        target,
        PropertyKeyRef::Units(&units),
    ))
}

#[cfg(test)]
pub(super) fn has_property(intrinsics: &Intrinsics, target: &Value, key: &Value) -> bool {
    has_property_checked(intrinsics, target, key).expect("test property-key conversion")
}

pub(super) fn has_property_key(
    intrinsics: &Intrinsics,
    target: &Value,
    key: PropertyKeyRef<'_>,
) -> bool {
    // A shared value (ADR 0073) answers `in` as the frozen object it is: its own fields, then the
    // chain its facade reports. It has no store of its own, so every arm below missed it and
    // `"a" in share({ a: 1 })` was false while the read answered 1.
    if let Value::Shared(facade) = target {
        return facade.own_property(key).is_some()
            || has_property_key(intrinsics, &facade.prototype(), key);
    }
    if let Value::Array(items) = target
        && let Some(index) = array_index_key(key)
    {
        if array_exotic_element(items, index).is_some()
            || items
                .borrow()
                .get(index)
                .is_some_and(|value| !matches!(value, Value::Native(Native::Hole, _)))
        {
            return true;
        }
        return array_inherited_index_key(intrinsics, items, key).is_some();
    }
    if let Value::TypedArray(view) = target
        && let Some(index) = canonical_numeric_index_key(key)
    {
        return valid_integer_index(view, index).is_some();
    }
    if let Value::String(text) = target
        && (key.matches_ascii("length")
            || array_index_key(key).is_some_and(|index| index < text.unit_len()))
    {
        return true;
    }
    let Some(name) = key.known_ascii() else {
        return match target {
            Value::Object(object) => object.borrow().lookup_key(key).is_some(),
            Value::Function(closure) => closure.properties.borrow().lookup_key(key).is_some(),
            Value::Array(items) => items.properties.borrow().lookup_key(key).is_some(),
            Value::Bound(bound) => bound.properties.borrow().lookup_key(key).is_some(),
            Value::Native(_, _) | Value::Callback(_, _) => {
                matches!(intrinsics.overridden_key(target, key), Overridden::Held(..))
            }
            other => exotic_properties(other)
                .is_some_and(|store| store.borrow().lookup_key(key).is_some()),
        };
    };
    has_property_ascii(intrinsics, target, name.as_ref())
}

pub(super) fn has_property_ascii(intrinsics: &Intrinsics, target: &Value, name: &str) -> bool {
    // `1 in [1, , 3]` is false and `1 in [1, undefined, 3]` is true: the one place the difference
    // between a hole and an undefined element is directly observable.
    if let Value::Array(items) = target
        && let Some(index) = array_index_key(PropertyKeyRef::Utf8(name))
    {
        if array_exotic_element(items, index).is_some() {
            return true;
        }
        if items
            .borrow()
            .get(index)
            .is_some_and(|value| !matches!(value, Value::Native(Native::Hole, _)))
        {
            return true;
        }
        // A hole is the absence of an own property, so the question carries on up the chain.
        return array_inherited_index(intrinsics, items, name).is_some();
    }
    // Every element key is the view's own answer, whether or not it reaches a slot: `"1.5" in
    // sample` is false because no view has such an element, not because the store is empty.
    if let Value::TypedArray(view) = target
        && let Some(index) = canonical_numeric_index(name)
    {
        return valid_integer_index(view, index).is_some();
    }
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
            object.borrow().lookup(&name).is_some()
                || string_exotic(target, &name).is_some()
                // The chain may end at a built-in's method table -- a wrapper's does, and so
                // does a class that extends one -- which `lookup` cannot walk into. `in` has to
                // agree with the read that does. `own_attributes` rather than the table's own
                // `property`, because a program may have *added* a name to `String.prototype`
                // and the table alone answers only for what it was built with: `new String()`
                // read `String.prototype.zz` correctly and denied it to `in`.
                || object
                    .borrow()
                    .prototype_table()
                    .is_some_and(|(table, realm)| {
                        own_attributes(
                            intrinsics,
                            &Value::Native(
                                Native::PrototypeOf(table),
                                realm.unwrap_or(RealmId::PRIMARY),
                            ),
                            &name,
                        )
                        .is_some()
                    })
                // An iterator this realm made inherits `%IteratorPrototype%`, as the read does.
                || ((object.borrow().get_own("@@iterated").is_some()
                    || object.borrow().get_own(HELPER_SOURCE).is_some())
                    && !matches!(
                        iterator_prototype_property(intrinsics, &name, value_realm(target)),
                        Value::Undefined
                    ))
                || !matches!(inherited_object_property(intrinsics, &name, value_realm(target)), Value::Undefined)
                // A property the program *defined* on `Object.prototype` that reads as
                // undefined is still there: an accessor given only a setter answers `undefined`
                // to a read and `true` to `in`, and asking the read whether it found anything
                // cannot tell those apart. Every generic array walk consults `HasProperty`, so
                // the difference decides whether an index is visited at all. Only the realm's
                // store is consulted, not the table beneath it: a module namespace has no
                // prototype at all, and the table would hand one `__proto__` and `toString`.
                || matches!(
                    intrinsics.overridden(
                        &Value::Native(Native::PrototypeOf(PrototypeTable::Object), RealmId::PRIMARY),
                        &name,
                    ),
                    Overridden::Held(..)
                )
        }
        Value::Array(items) => {
            if let Some(index) = array_index_key(PropertyKeyRef::Utf8(name)) {
                return index < items.borrow().len();
            }
            // An array carries a property store beside its elements -- `arr.value = 1` goes
            // there -- and it has to be consulted first, or `in` disagrees with the read that
            // finds it. Methods and `length` live on the prototype in a real engine; here the
            // property lookup answers for both, so `in` has to agree with that too.
            items.properties.borrow().lookup(&name).is_some()
                || name == "length"
                || !matches!(array_property(&name), Value::Undefined)
                // A name the program put on `Array.prototype` is on the chain too, and only the
                // realm's store knows about it. Without this step `Array.prototype.value = 1`
                // read through every array and answered `false` to `"value" in arr` -- which is
                // the question `ToPropertyDescriptor` asks of an array used as a descriptor.
                || own_attributes(
                    intrinsics,
                    &Value::Native(Native::PrototypeOf(PrototypeTable::Array), RealmId::PRIMARY),
                    &name,
                )
                .is_some()
                || !matches!(
                    inherited_object_property(intrinsics, &name, value_realm(target)),
                    Value::Undefined
                )
        }
        // Every element key was answered above, so what is left is a named one: the store
        // beside the view first, so `in` agrees with the read that finds it, and then the chain.
        Value::TypedArray(view) => {
            view.properties.borrow().lookup(&name).is_some()
                || matches!(
                    name,
                    "length" | "byteLength" | "byteOffset" | "BYTES_PER_ELEMENT" | "buffer"
                )
                // The same two links the read walks, so `in` and the read cannot disagree about
                // a name a program put on either view prototype.
                || own_attributes(
                    intrinsics,
                    &Value::Native(Native::PrototypeOf(PrototypeTable::TypedArrayFor(view.kind)), RealmId::PRIMARY),
                    &name,
                )
                .is_some()
                || own_attributes(
                    intrinsics,
                    &Value::Native(Native::PrototypeOf(PrototypeTable::TypedArray), RealmId::PRIMARY),
                    &name,
                )
                .is_some()
                || !matches!(
                    inherited_object_property(intrinsics, &name, value_realm(target)),
                    Value::Undefined
                )
        }
        // A buffer, a view and a regexp answer from their store and then from the prototype
        // their kind links to, which is where every accessor and method they publish lives.
        // Answering `false` for all of them made `"byteLength" in buffer` deny a property the
        // read right beside it produces.
        other if exotic_properties(other).is_some() => {
            exotic_properties(target).is_some_and(|store| store.borrow().lookup(&name).is_some())
                || exotic_reserved_name(target, &name)
                || own_attributes(intrinsics, &prototype_of(target), &name).is_some()
                || !matches!(
                    inherited_object_property(intrinsics, &name, value_realm(target)),
                    Value::Undefined
                )
        }
        // Mirrors the read: the realm's store first, then the table, then the `name` and
        // `length` every callable owns. A property a program deleted is absent here too, or
        // `delete` and the `in` after it would disagree.
        Value::Native(native, _) => {
            let own = match intrinsics.overridden(target, &name) {
                Overridden::Held(..) => true,
                Overridden::Absent => false,
                Overridden::Untouched => {
                    // `intrinsic_owns` rather than `property`, which is what a *read* finds and
                    // is a different question. A prototype table's `property` carries its last
                    // step up to `Object.prototype` and answers `Some(Undefined)` for a name it
                    // does not have at all, so asking it made `in` true for every string on
                    // every built-in prototype in the realm -- `"nope" in Array.prototype` among
                    // them, which is worse than useless to the feature-detects that ask it. The
                    // ownership test is the one `hasOwnProperty` and `getOwnPropertyNames`
                    // already agree on; the step up to `Object.prototype` is taken below, so
                    // `"hasOwnProperty" in Array.prototype` stays true.
                    intrinsic_owns(*native, &name).is_some()
                        || identity_property(target, &name).is_some()
                        || (matches!(native, Native::Error(kind) if *kind != ErrorKind::Error)
                            && name == "isError"
                            && !matches!(
                                intrinsics.overridden(
                                    &Value::Native(
                                        Native::Error(ErrorKind::Error),
                                        RealmId::PRIMARY
                                    ),
                                    &name,
                                ),
                                Overridden::Absent
                            ))
                }
            };
            // `Math` and `JSON` are ordinary objects that inherit from `Object.prototype`, and so
            // is every constructor and method table but that one. Answering from the built-in's
            // own names alone denied them `hasOwnProperty`, and denied `Math` a property the
            // program had put on `Object.prototype` and could read straight off it.
            let inherited = if matches!(
                native,
                Native::PrototypeOf(PrototypeTable::IteratorFor(_) | PrototypeTable::Generator)
            ) {
                // One link further: these reach `%IteratorPrototype%` before `Object.prototype`.
                native_inherited_property(intrinsics, target, &name)
            } else {
                inherited_object_property(intrinsics, &name, value_realm(target))
            };
            own || (!matches!(native, Native::PrototypeOf(PrototypeTable::Object))
                && !matches!(inherited, Value::Undefined))
        }
        // Callable methods belong to the actual prototype chain. In particular, setting a
        // function's prototype to null removes inherited methods without removing its own
        // identity properties. Membership must not invoke an inherited accessor.
        Value::Function(closure) => {
            closure.properties.borrow().lookup(&name).is_some()
                || matches!(name, "name" | "length")
                || (name == "prototype" && has_prototype_property(closure))
                || has_property_ascii(intrinsics, &prototype_of(target), name)
        }
        _ => false,
    }
}

#[derive(Clone, Debug)]
pub struct CollectionKey(pub Value);

impl PartialEq for CollectionKey {
    fn eq(&self, other: &Self) -> bool {
        same_value_zero(&self.0, &other.0)
    }
}

impl Eq for CollectionKey {}

impl std::hash::Hash for CollectionKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(&self.0).hash(state);
        match &self.0 {
            Value::Undefined | Value::Null => {}
            Value::Boolean(b) => b.hash(state),
            Value::Number(n) => {
                if n.is_nan() {
                    0x7ff8_0000_0000_0000_u64.hash(state);
                } else if *n == 0.0 {
                    0.0_f64.to_bits().hash(state);
                } else {
                    n.to_bits().hash(state);
                }
            }
            Value::String(s) => s.hash(state),
            Value::BigInt(b) => b.hash(state),
            Value::Symbol(Symbol::WellKnown(wk)) => {
                std::mem::discriminant(wk).hash(state);
            }
            Value::Symbol(Symbol::Unique(u)) => {
                (Rc::as_ptr(u) as usize).hash(state);
            }
            Value::Native(nat, realm) => {
                std::mem::discriminant(nat).hash(state);
                realm.hash(state);
            }
            Value::Callback(cb, realm) => {
                std::mem::discriminant(cb).hash(state);
                realm.hash(state);
            }
            Value::Settler(p, one) => {
                (Rc::as_ptr(p) as usize).hash(state);
                one.hash(state);
            }
            Value::Cell(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::Generator(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::Promise(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::RegExp(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::Date(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::Temporal(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::Intl(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::Capability(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::Element(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::Array(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::Shared(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::TypedArray(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::ArrayBuffer(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::DataView(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::Proxy(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::Bound(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::Wrapped(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::CommonJsRequire(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::CommonJsResolve(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::Object(a) => (Rc::as_ptr(a) as usize).hash(state),
            Value::Function(a) => (Rc::as_ptr(a) as usize).hash(state),
        }
    }
}

/// Where a collection's backing entries hold this key, or `None`.
#[allow(clippy::mutable_key_type)]
pub(super) fn find_collection_entry(
    entries: &Rc<ArrayData>,
    key: &Value,
    is_map: bool,
) -> Option<usize> {
    if entries.borrow().len() < 16 {
        return find_entry(&entries.borrow(), key, is_map);
    }
    let mut index_opt = entries.collection_index.borrow_mut();
    let index_map = index_opt.get_or_insert_with(|| {
        let mut map = std::collections::HashMap::new();
        for (idx, entry) in entries.borrow().iter().enumerate() {
            match entry {
                Value::Native(Native::Tombstone, _) => {}
                Value::Array(pair) if is_map => {
                    if let Some(k) = pair.borrow().first() {
                        map.entry(CollectionKey(k.clone())).or_insert(idx);
                    }
                }
                other if !is_map => {
                    map.entry(CollectionKey(other.clone())).or_insert(idx);
                }
                _ => {}
            }
        }
        map
    });
    index_map.get(&CollectionKey(key.clone())).copied()
}

#[allow(clippy::mutable_key_type)]
pub(super) fn collection_entry_inserted(entries: &Rc<ArrayData>, key: &Value, index: usize) {
    if let Some(index_map) = entries.collection_index.borrow_mut().as_mut() {
        index_map.insert(CollectionKey(key.clone()), index);
    }
}

#[allow(clippy::mutable_key_type)]
pub(super) fn collection_entry_deleted(entries: &Rc<ArrayData>, key: &Value) {
    if let Some(index_map) = entries.collection_index.borrow_mut().as_mut() {
        index_map.remove(&CollectionKey(key.clone()));
    }
}

#[allow(clippy::mutable_key_type)]
pub(super) fn collection_cleared(entries: &Rc<ArrayData>) {
    if let Some(index_map) = entries.collection_index.borrow_mut().as_mut() {
        index_map.clear();
    }
}

/// The properties an array exposes, shared by reads and by `in` so the two cannot disagree.
/// The index of `key` among live entries, or `None`.
pub(super) fn find_entry(entries: &[Value], key: &Value, is_map: bool) -> Option<usize> {
    entries.iter().position(|entry| match entry {
        Value::Native(Native::Tombstone, _) => false,
        Value::Array(pair) if is_map => pair
            .borrow()
            .first()
            .is_some_and(|existing| same_value_zero(existing, key)),
        other => !is_map && same_value_zero(other, key),
    })
}

/// Where a collection keeps its count.
///
/// Reserved rather than spelled `size`, because a plain property of that name *is* an own
/// property: `Object.getOwnPropertyNames(new Map())` answered `["size"]`, which is a property
/// no Map has. The count is reported by the accessor on `Map.prototype`, where the
/// specification puts it.
pub(super) const COLLECTION_SIZE: &str = "@@size";

/// Adjust a collection's stored size.
///
/// Stored rather than derived so a read is a plain slot lookup; the two cannot drift because
/// only `add` and `delete` change either.
pub(super) fn set_size(receiver: &Value, size: f64) {
    if let Value::Object(object) = receiver {
        object
            .borrow_mut()
            .set(COLLECTION_SIZE, Value::Number(size));
    }
}

pub(super) fn bump_size(receiver: &Value, delta: f64) {
    if let Value::Object(object) = receiver {
        let current = object
            .borrow()
            .get_own(COLLECTION_SIZE)
            .map_or(0.0, Value::as_number);
        object
            .borrow_mut()
            .set(COLLECTION_SIZE, Value::Number(current + delta));
    }
}
