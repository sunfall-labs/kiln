use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// Resolve a VM-managed property without invoking a program-defined accessor.
    pub(super) fn property(&self, target: &Value, name: &str) -> Value {
        // Every intrinsic kind answers `constructor` with the built-in that makes it. Handled
        // here rather than in each table because the answer is the same question for all of
        // them, and because `new x.constructor(...)` is how a library clones a value whose type
        // it was handed rather than told.
        //
        // Anything the *chain* says wins, because a `constructor` reached that way is a real
        // property: `a.constructor = {}` is an ordinary assignment and reading it back has to
        // see what was written, and `class MyArray extends Array {}` puts its own on its
        // prototype. Both are exactly what `ArraySpeciesCreate` consults, and answering the
        // intrinsic over them made every subclass's derived methods build a plain array.
        if name == "constructor"
            && own_property_state(&self.intrinsics, target, name).is_none()
            && self.stored_property(target, name).is_none()
            && let Some(constructor) = intrinsic_constructor(target)
        {
            return constructor;
        }
        // An element, a typed-array slot and a string's code unit are storage rather than
        // properties, and a name is how a pattern reaches one: `var { 0: v } = [7]` binds the
        // element, not a property called "0". The named and computed spellings of one key cannot
        // disagree about what they reach, so the named read routes to the same place.
        if matches!(
            target,
            Value::Array(_) | Value::TypedArray(_) | Value::String(_)
        ) && canonical_index(name).is_some()
        {
            return self.computed_key_ref(target, PropertyKeyRef::Utf8(name));
        }
        // An exotic object is an ordinary object for every name its own behaviour does not
        // claim, and an own property shadows the prototype's exactly as it does anywhere else:
        // `sample.constructor = C` has to read back as `C` rather than as the view's own kind.
        if let Some(store) = exotic_properties(target)
            && !exotic_reserved_name(target, name)
            && let Some(found) = store.borrow().get(name)
        {
            return found;
        }
        match target {
            // A `Temporal` value's fields and methods both come from its own table: `day` is a
            // question only the calendar engine can answer, and which methods exist depends on
            // which of the eight types this is.
            Value::Temporal(built) => {
                temporal::instance_property(built.value(), name).unwrap_or(Value::Undefined)
            }
            // A `DisposableStack` is an object carrying reserved state too, and `disposed` is
            // read straight off it. The accessor on the prototype is what
            // `getOwnPropertyDescriptor` reports; this is what the read answers, for the same
            // reason a collection's `size` is answered here -- an instance is an ordinary object
            // and the chain walk below would find the getter rather than run it.
            Value::Object(object)
                if name == "disposed"
                    && object.borrow().get_own(ASYNC_DISPOSE_ENTRIES).is_some() =>
            {
                Value::Boolean(
                    object
                        .borrow()
                        .get_own(ASYNC_DISPOSE_DONE)
                        .is_some_and(Value::is_truthy),
                )
            }
            Value::Object(object)
                if name == "disposed" && object.borrow().get_own(DISPOSE_ENTRIES).is_some() =>
            {
                Value::Boolean(
                    object
                        .borrow()
                        .get_own(DISPOSE_DONE)
                        .is_some_and(Value::is_truthy),
                )
            }
            // A Set or Map instance is an object whose reserved state marks it. Its methods come
            // from one table shared with `in`, so the two cannot disagree -- and this arm has to
            // precede the ordinary object case, which would otherwise answer `undefined` first.
            Value::Object(object) if object.borrow().get_own("@@entries").is_some() => {
                let (is_map, weak) = {
                    let object = object.borrow();
                    (
                        matches!(object.get_own("@@map"), Some(&Value::Boolean(true))),
                        matches!(object.get_own("@@weak"), Some(&Value::Boolean(true))),
                    )
                };
                // `size` is an ordinary stored property for a strong collection and absent for a
                // weak one, which has no count to report.
                if name == "size" {
                    if weak {
                        return Value::Undefined;
                    }
                    return object
                        .borrow()
                        .get(COLLECTION_SIZE)
                        .unwrap_or(Value::Number(0.0));
                }
                // Through the realm's store first, so that a program which replaced -- or
                // removed -- a method on `Map.prototype` sees its change from an instance too.
                // Without it the instance answered from the table the program had already
                // edited, and the prototype and the instance disagreed.
                let table = match (is_map, weak) {
                    (true, false) => PrototypeTable::Map,
                    (false, false) => PrototypeTable::Set,
                    (true, true) => PrototypeTable::WeakMap,
                    (false, true) => PrototypeTable::WeakSet,
                };
                let fallback = match (is_map, weak) {
                    (true, false) => tabulated_map_property(name),
                    (false, false) => tabulated_set_property(name),
                    (true, true) => tabulated_weak_map_property(name),
                    (false, true) => tabulated_weak_set_property(name),
                };
                match table_step(&self.intrinsics, table, name, fallback, value_realm(target)) {
                    Value::Undefined => object.borrow().get(name).unwrap_or_else(|| {
                        inherited_object_property(&self.intrinsics, name, value_realm(target))
                    }),
                    method => method,
                }
            }
            // An iterator this engine made -- an array or string walk, a collection walk, or a
            // helper built by one of the methods above -- inherits `%IteratorPrototype%`. A
            // plain object that merely has a `next` does not, which is the distinction the
            // specification draws and the reason this asks for the marker rather than the shape.
            Value::Object(object)
                if object.borrow().get_own("@@iterated").is_some()
                    || object.borrow().get_own(HELPER_SOURCE).is_some() =>
            {
                let object = object.borrow();
                let from_prototype = || match object.prototype() {
                    // The instance's prototype is `%ArrayIteratorPrototype%` or one of its kin,
                    // which are markers rather than objects -- so the store's own chain walk
                    // stops at them and the tag they publish never arrives. Read as the marker
                    // itself is read, so the realm's store is consulted before its table.
                    Some(prototype @ Value::Native(..)) => match self.property(&prototype, name) {
                        Value::Undefined => None,
                        found => Some(found),
                    },
                    _ => None,
                };
                object.get(name).or_else(from_prototype).unwrap_or_else(|| {
                    // `%IteratorPrototype%` sits *above* `Object.prototype` rather than
                    // replacing it, so a name it does not answer still reaches the ordinary
                    // object methods -- which is what stops adding the helpers from taking
                    // `hasOwnProperty` away from every iterator in the realm.
                    match iterator_prototype_property(&self.intrinsics, name, value_realm(target)) {
                        Value::Undefined => {
                            inherited_object_property(&self.intrinsics, name, value_realm(target))
                        }
                        found => found,
                    }
                })
            }
            // A String wrapper's characters and `length` come before anything the chain could
            // answer, because they are own properties of the box.
            Value::Object(_) if string_exotic(target, name).is_some() => {
                string_exotic(target, name).unwrap_or(Value::Undefined)
            }
            Value::Object(object) => {
                let object = object.borrow();
                if let Some(NodeObject::HttpResponse(state)) = &object.node_object {
                    return match name {
                        "statusCode" => Value::Number(f64::from(state.borrow().status)),
                        "headersSent" => Value::Boolean(state.borrow().streaming),
                        "writableEnded" | "finished" => Value::Boolean(state.borrow().ended),
                        _ => object.get(name).unwrap_or_else(|| {
                            inherited_object_property(&self.intrinsics, name, value_realm(target))
                        }),
                    };
                }
                if let Some(NodeObject::Url(state)) = &object.node_object {
                    return url::read_property(&object, state, name);
                }
                object.get(name).unwrap_or_else(|| {
                    // An object whose prototype is explicitly `null` inherits nothing --
                    // `Object.create(null)` is how a program asks for a bare dictionary, and
                    // handing it `hasOwnProperty` would defeat the point.
                    if matches!(object.prototype(), Some(Value::Null)) {
                        Value::Undefined
                    } else if let Some(found) = object
                        .prototype_table()
                        .and_then(|(table, realm)| {
                            // The realm's store in front of the table, so an object whose chain
                            // ends at `Array.prototype` finds what a program put there.
                            let realm = realm.unwrap_or_else(|| value_realm(target));
                            Some(table_step(
                                &self.intrinsics,
                                table,
                                name,
                                Native::PrototypeOf(table).property(name)?,
                                realm,
                            ))
                        })
                        .filter(|found| !matches!(found, Value::Undefined))
                    {
                        // The chain reaches a built-in's method table, which `lookup` cannot
                        // walk into. Answering from it here is what an object created by
                        // extending a built-in, or pointed at one with `setPrototypeOf`,
                        // inherits.
                        found
                    } else if name == "constructor" {
                        // The end of the chain is `Object.prototype`, whose `constructor` is
                        // `Object`. Answered here rather than beside the other intrinsics
                        // because a class instance finds its own on the way down, and this must
                        // not shadow it: `x.constructor === Object` is how a library asks
                        // whether something is a *plain* object.
                        Value::Native(Native::ObjectConstructor, RealmId::PRIMARY)
                    } else {
                        inherited_object_property(&self.intrinsics, name, value_realm(target))
                    }
                })
            }
            // The table first, then the `name`/`length` every callable owns, then whatever
            // `Function.prototype` supplies. A table that answered `undefined` has still
            // answered -- `Object.prototype.call` is not a function -- so the fallback to
            // `function_property` applies only where there was no arm at all.
            // The realm's store first, because a program's change to a built-in has to be
            // what the next read finds; then the table, then the `name`/`length` every callable
            // owns, then whatever `Function.prototype` supplies. A table that answered
            // `undefined` has still answered -- `Object.prototype.call` is not a function -- so
            // the fallback to `function_property` applies only where there was no arm at all.
            // A table answer names the *receiver's* realm: `other.Array.prototype` is realm 1's
            // array prototype, not this one's. The tables cannot say so themselves -- they are
            // pure functions of a `Copy` marker and a name, with no receiver to ask -- so what
            // comes back is retagged here, where the receiver is in hand. A value the program
            // *stored* is returned untouched: it keeps the realm it already had.
            Value::Native(native, realm) => match self.intrinsics.overridden(target, name) {
                Overridden::Held(Property::Value(value), _) => value,
                // A getter is ordinary code and this path cannot call one; `read_property`
                // looks for the accessor before it gets here.
                Overridden::Held(Property::Accessor { .. }, _) => Value::Undefined,
                // Deleted, so the read carries on *up* the chain rather than back into the
                // table that still has the answer the program removed.
                Overridden::Absent => native_inherited_property(&self.intrinsics, target, name),
                // An error constructor's `prototype` is a per-realm object rather than a table
                // entry, so `native.property` has nothing to answer with and the read used to
                // walk on up to `Function.prototype` and come back `undefined`. Only the named
                // spelling was ever right, and only because one instruction special-cased it:
                // `Error["prototype"]` and `Reflect.get(Error, "prototype")` were both undefined
                // while `Error.prototype` was an object.
                Overridden::Untouched
                    if matches!(native, Native::Error(_)) && name == "prototype" =>
                {
                    let Native::Error(kind) = native else {
                        unreachable!("checked by the guard");
                    };
                    self.intrinsics.error_prototype(*kind, *realm)
                }
                Overridden::Untouched
                    if matches!(native, Native::PrototypeOf(PrototypeTable::Array))
                        && name == WellKnown::Unscopables.property_key() =>
                {
                    self.intrinsics.array_unscopables(*realm)
                }
                Overridden::Untouched => {
                    match native.property(name).map(|found| in_realm(found, *realm)) {
                        Some(found) if !matches!(found, Value::Undefined) => found,
                        // `String.prototype` boxes the empty string, so it owns a `length` of 0 the
                        // same way any other String object does.
                        _ if string_exotic(target, name).is_some() => {
                            string_exotic(target, name).unwrap_or(Value::Undefined)
                        }
                        Some(Value::Undefined) => {
                            native_inherited_property(&self.intrinsics, target, name)
                        }
                        Some(found) => found,
                        None => identity_property(target, name).unwrap_or_else(|| {
                            native_inherited_property(&self.intrinsics, target, name)
                        }),
                    }
                }
            },
            // `f.prototype` is the object `new f()` gives instances. Reading it creates it, so
            // that `f.prototype.m = ...` before any `new` reaches the same object the instances
            // will see.
            Value::Function(closure) if name != "prototype" => {
                closure
                    .properties
                    .borrow()
                    .get(name)
                    // A subclass of a built-in inherits that built-in's statics, which is the
                    // whole of what `class P extends Promise {}` does to the two *constructors*.
                    // The link is an ordinary prototype pointer, but what it points at is an
                    // intrinsic with no property store -- so the store walk above runs out and
                    // `P.resolve`, `P.all` and `A.from` all read as `undefined`. Asked of the
                    // built-in rather than of a table, so a program that replaced
                    // `Promise.resolve` is seen from the subclass too.
                    .or_else(|| {
                        inherited_builtin(closure)
                            .map(|native| {
                                self.property(&Value::Native(native, RealmId::PRIMARY), name)
                            })
                            .filter(|found| !matches!(found, Value::Undefined))
                    })
                    // No fallback for `name` or `length`: both are stored on the closure, and
                    // answering them from the proto as well would resurrect one a program had
                    // just deleted -- which is precisely the observation `configurable: true`
                    // promises it can make.
                    .unwrap_or_else(|| {
                        function_property(&self.intrinsics, name, value_realm(target))
                    })
            }
            // A bound function reports the arity still to be supplied and the name of what it
            // wraps, which is what makes a stack trace through one readable.
            // A bound function reports the arity still to be supplied and prefixes the name of
            // what it wraps, which is what makes a stack trace through one readable.
            //
            // It owns no `prototype` (BoundFunctionCreate defines none), so the name is inherited
            // from its [[Prototype]], which is its target's: a bound derived class answers with
            // its parent's `prototype`, an ordinary function's with nothing (issue 28).
            Value::Bound(bound) => bound
                .properties
                .borrow()
                .get(name)
                .or_else(|| identity_property(target, name))
                .unwrap_or_else(|| {
                    if name == "prototype" {
                        return self.property(&prototype_of(&bound.target), name);
                    }
                    function_property(&self.intrinsics, name, value_realm(target))
                }),
            // A wrapped function answers from its own store first -- which is where the `name`
            // and `length` copied off the target live -- and from `Function.prototype`
            // otherwise, so `call`, `apply`, `bind` and `toString` all work on one. No
            // `identity_property`: unlike every other callable here, this one's `name` and
            // `length` are real stored properties rather than derived ones, and deriving them
            // would answer about the wrapper instead of about what it wraps.
            Value::Wrapped(wrapped) => {
                wrapped.properties.borrow().get(name).unwrap_or_else(|| {
                    function_property(&self.intrinsics, name, value_realm(target))
                })
            }
            Value::CommonJsRequire(require) if name == "resolve" => {
                Value::CommonJsResolve(Rc::clone(require))
            }
            Value::CommonJsRequire(_) | Value::CommonJsResolve(_) => {
                identity_property(target, name).unwrap_or_else(|| {
                    function_property(&self.intrinsics, name, value_realm(target))
                })
            }
            // Every other callable answers to the same three, and to nothing else: none of them
            // has a property store to shadow them with.
            Value::Callback(_, _)
            | Value::Settler(..)
            | Value::Capability(..)
            | Value::Element(_) => match self.intrinsics.overridden(target, name) {
                Overridden::Held(Property::Value(value), _) => value,
                Overridden::Held(Property::Accessor { .. }, _) => Value::Undefined,
                Overridden::Absent => {
                    function_property(&self.intrinsics, name, value_realm(target))
                }
                Overridden::Untouched => identity_property(target, name).unwrap_or_else(|| {
                    function_property(&self.intrinsics, name, value_realm(target))
                }),
            },
            // Gated on the same predicate as the descriptor, or the two disagree: a function
            // whose `hasOwnProperty("prototype")` is false must not answer an object when the
            // property is read by name. That split is what an earlier version of this produced.
            Value::Function(closure) if name == "prototype" && has_prototype_property(closure) => {
                prototype_object(closure)
            }
            // `length` is not an ordinary property of an array: it is derived, and writing it
            // truncates. Reading it is all the corpus needs so far.
            Value::Array(items) if name == "length" => Value::Number(items.length() as f64),
            // Unbound, so that reading the same method twice yields the same value. The
            // receiver is supplied by `CallMethod`, not captured here.
            // Strings expose `length` and the iterator. `length` is UTF-16 code units, which is
            // what JavaScript counts, and is not the same as either bytes or characters.
            Value::String(text) if name == "length" => {
                Value::Number(text.encode_utf16().count() as f64)
            }
            Value::String(_) => match table_step(
                &self.intrinsics,
                PrototypeTable::String,
                name,
                string_property(name),
                value_realm(target),
            ) {
                Value::Undefined => {
                    inherited_object_property(&self.intrinsics, name, value_realm(target))
                }
                method => method,
            },
            Value::Number(_) => match table_step(
                &self.intrinsics,
                PrototypeTable::Number,
                name,
                number_property(name),
                value_realm(target),
            ) {
                Value::Undefined => {
                    inherited_object_property(&self.intrinsics, name, value_realm(target))
                }
                method => method,
            },
            // The array's own named properties, then `Array.prototype`, then
            // `Object.prototype`: the chain an array actually has, walked in the order it has
            // it. The built-in methods used to come first, which was invisible while nothing
            // could be written to `Array.prototype` -- now that something can, an own property
            // has to shadow the prototype's and a replaced `toString` has to be the one an
            // array finds.
            Value::Array(items) => items.properties.borrow().get(name).unwrap_or_else(|| {
                match table_step(
                    &self.intrinsics,
                    PrototypeTable::Array,
                    name,
                    array_property(name),
                    value_realm(target),
                ) {
                    // Past `Array.prototype` the chain reaches `Object.prototype`, which is
                    // where `hasOwnProperty` lives -- and reaching it is what the plain-object
                    // check every library ships depends on.
                    Value::Undefined => {
                        inherited_object_property(&self.intrinsics, name, value_realm(target))
                    }
                    method => method,
                }
            }),
            // A regexp's own properties. `lastIndex` is the mutable one, and the reason two
            // evaluations of one literal must not share an object.
            // Through the realm's store first, like every other primitive: a program that puts
            // a property on `Boolean.prototype` has to find it from `true`, which is the whole
            // point of a wrapper prototype. Without the store these three answered only from
            // their tables, so `BigInt.prototype.x = 1; (1n).x` was undefined.
            Value::Boolean(_) => match table_step(
                &self.intrinsics,
                PrototypeTable::Boolean,
                name,
                boolean_method(name),
                value_realm(target),
            ) {
                Value::Undefined => {
                    inherited_object_property(&self.intrinsics, name, value_realm(target))
                }
                method => method,
            },
            Value::BigInt(_) => match table_step(
                &self.intrinsics,
                PrototypeTable::BigInt,
                name,
                bigint_method(name),
                value_realm(target),
            ) {
                Value::Undefined => {
                    inherited_object_property(&self.intrinsics, name, value_realm(target))
                }
                method => method,
            },
            // A symbol reaches `Symbol.prototype` like any other primitive. `description` is the
            // one property that is not a method, and it distinguishes an absent description from
            // an empty one -- `Symbol().description` is undefined and `Symbol("").description`
            // is `""`, which is the only way a program can tell the two calls apart.
            Value::Symbol(_) if name == "description" => {
                builtin_accessor_slot(BuiltinAccessor::SymbolDescription, target)
            }
            Value::Symbol(_) => match table_step(
                &self.intrinsics,
                PrototypeTable::Symbol,
                name,
                Native::PrototypeOf(PrototypeTable::Symbol)
                    .property(name)
                    .unwrap_or(Value::Undefined),
                value_realm(target),
            ) {
                Value::Undefined => {
                    inherited_object_property(&self.intrinsics, name, value_realm(target))
                }
                method => method,
            },
            // Through the realm's store first, so that a program which replaced a method on
            // `Date.prototype` sees its own from an instance, and then up to `Object.prototype`.
            Value::Date(_) => match table_step(
                &self.intrinsics,
                PrototypeTable::Date,
                name,
                date_method(name),
                value_realm(target),
            ) {
                Value::Undefined => {
                    inherited_object_property(&self.intrinsics, name, value_realm(target))
                }
                method => method,
            },
            // A Buffer reads its members off `Buffer.prototype` first; `length` and the rest
            // of the view's own accessors are below and the prototype defines none of them.
            Value::TypedArray(view)
                if is_buffer_view(view) && buffer_prototype_own(name).is_some() =>
            {
                buffer_prototype_own(name).unwrap_or(Value::Undefined)
            }
            Value::TypedArray(view) => match name {
                "length" => builtin_accessor_slot(BuiltinAccessor::TypedArrayLength, target),
                "byteLength" => {
                    builtin_accessor_slot(BuiltinAccessor::TypedArrayByteLength, target)
                }
                // Zero, not the recorded offset, once a shrink has left the window outside the
                // buffer: the specification reports an out-of-bounds view as having no position
                // at all, and an offset into bytes that are gone is worse than no offset.
                "byteOffset" => {
                    builtin_accessor_slot(BuiltinAccessor::TypedArrayByteOffset, target)
                }
                "BYTES_PER_ELEMENT" => Value::Number(view.kind.width() as f64),
                "buffer" => builtin_accessor_slot(BuiltinAccessor::TypedArrayBuffer, target),
                "constructor" => {
                    Value::Native(Native::TypedArrayConstructor(view.kind), RealmId::PRIMARY)
                }
                key if key == WellKnown::ToStringTag.property_key() => {
                    Value::String(view.kind.name().to_owned().into())
                }
                "toString" if view.kind == TypedKind::Uint8 && is_buffer_view(view) => {
                    Value::Native(Native::NodeBufferToString, RealmId::PRIMARY)
                }
                // The chain a view actually has, walked in the order it has it:
                // `Int8Array.prototype`, then `%TypedArray%.prototype` where the shared methods
                // live, then `Object.prototype`. Each step asks the realm's store before its
                // table, so a program that put a property on any of the three -- or replaced a
                // method on one -- sees its change from an instance. Answering straight from
                // the shared table skipped the first two links entirely, and
                // `TypedArray.prototype.foo = 1` was invisible to every view in the realm.
                other => match table_step(
                    &self.intrinsics,
                    PrototypeTable::TypedArrayFor(view.kind),
                    other,
                    typed_array_for_method(view.kind, other),
                    value_realm(target),
                ) {
                    Value::Undefined => match table_step(
                        &self.intrinsics,
                        PrototypeTable::TypedArray,
                        other,
                        Value::Undefined,
                        value_realm(target),
                    ) {
                        Value::Undefined => {
                            inherited_object_property(&self.intrinsics, other, value_realm(target))
                        }
                        found => found,
                    },
                    found => found,
                },
            },
            Value::ArrayBuffer(buffer) => match name {
                "byteLength" => builtin_accessor_slot(BuiltinAccessor::BufferByteLength, target),
                // Each buffer type answers only its own name for the ceiling's meaning. A
                // shared buffer has no `resizable` and an unshared one no `growable`: the
                // specification makes each accessor a `TypeError` on the other type, and
                // answering `false` instead would tell a program that a growable buffer cannot
                // grow.
                "resizable" if !buffer.shared => {
                    builtin_accessor_slot(BuiltinAccessor::BufferResizable, target)
                }
                "growable" if buffer.shared => {
                    builtin_accessor_slot(BuiltinAccessor::SharedGrowable, target)
                }
                // A fixed-length buffer answers its own length rather than zero or `undefined`:
                // the accessor is defined for every buffer, and "the most this can ever hold"
                // is a question a fixed buffer has an answer to.
                "maxByteLength" => {
                    builtin_accessor_slot(BuiltinAccessor::BufferMaxByteLength, target)
                }
                // A shared buffer has no `detached` at all -- nothing can take bytes away from
                // a holder that has no way to learn it happened, which is the same reason it has
                // no `resize`.
                "detached" if !buffer.shared => {
                    builtin_accessor_slot(BuiltinAccessor::BufferDetached, target)
                }
                "immutable" if !buffer.shared => {
                    builtin_accessor_slot(BuiltinAccessor::BufferImmutable, target)
                }
                "constructor" if buffer.shared => {
                    Value::Native(Native::SharedArrayBufferConstructor, RealmId::PRIMARY)
                }
                "constructor" => Value::Native(Native::ArrayBufferConstructor, RealmId::PRIMARY),
                // Through the realm's store and then the table, and on to `Object.prototype`
                // above it -- the chain a buffer actually has. Stopping at the table left
                // `buffer.hasOwnProperty` undefined, and a change to `ArrayBuffer.prototype`
                // invisible to every buffer in the realm.
                other => match table_step(
                    &self.intrinsics,
                    if buffer.shared {
                        PrototypeTable::SharedArrayBuffer
                    } else {
                        PrototypeTable::ArrayBuffer
                    },
                    other,
                    array_buffer_method(buffer.shared, other),
                    value_realm(target),
                ) {
                    Value::Undefined => {
                        inherited_object_property(&self.intrinsics, other, value_realm(target))
                    }
                    method => method,
                },
            },
            Value::DataView(_) => match name {
                "buffer" => builtin_accessor_slot(BuiltinAccessor::DataViewBuffer, target),
                // Zero for a stranded view, which is not what the specification says -- it says
                // `TypeError`. Only `read_property` can raise one, so the guard lives there and
                // this arm answers the machinery that reaches a property without being able to
                // throw at all.
                "byteLength" => builtin_accessor_slot(BuiltinAccessor::DataViewByteLength, target),
                "byteOffset" => builtin_accessor_slot(BuiltinAccessor::DataViewByteOffset, target),
                "constructor" => Value::Native(Native::DataViewConstructor, RealmId::PRIMARY),
                other => match table_step(
                    &self.intrinsics,
                    PrototypeTable::DataView,
                    other,
                    data_view_method(other),
                    value_realm(target),
                ) {
                    Value::Undefined => {
                        inherited_object_property(&self.intrinsics, other, value_realm(target))
                    }
                    method => method,
                },
            },
            Value::RegExp(regexp) => match name {
                "test" => Value::Native(Native::RegExpTest, RealmId::PRIMARY),
                "exec" => Value::Native(Native::RegExpExec, RealmId::PRIMARY),
                // Its own, not the error one: a regexp prints as `/source/flags`, and reusing
                // `Error.prototype.toString` made every regexp stringify as "Error".
                "toString" => Value::Native(Native::RegExpToString, RealmId::PRIMARY),
                // Escaped, exactly as the accessor on the prototype escapes it: the two must
                // not disagree about what `re.source` is, and the raw pattern would not survive
                // being written back between slashes.
                "source" => Value::String(escaped_source_units(&regexp.source)),
                // The specification's order -- `dgimsuvy` -- not the order the program wrote
                // them: `new RegExp("a", "yigm").flags` is "gimy". Answering the stored spelling
                // made the getter disagree with the generic one on `RegExp.prototype`, which
                // builds the string from the eight flag accessors in exactly this order.
                "flags" => Value::String(canonical_regexp_flags(&regexp.flags).into()),
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
                // Everything else -- the five symbol-keyed methods included -- through the
                // realm's store and then the table, so an instance and `RegExp.prototype` can
                // never resolve a name differently.
                other => match table_step(
                    &self.intrinsics,
                    PrototypeTable::RegExp,
                    other,
                    regexp_method(other),
                    value_realm(target),
                ) {
                    Value::Undefined => {
                        inherited_object_property(&self.intrinsics, other, value_realm(target))
                    }
                    method => method,
                },
            },
            // An `Intl` instance answers from the same table its prototype does, so the two
            // cannot resolve a name differently -- and `Intl.Locale`'s accessors are plain reads
            // here, as every other built-in getter in this interpreter is.
            Value::Intl(value) => match intl::instance_property(value, name) {
                Some(found) => found,
                None => inherited_object_property(&self.intrinsics, name, value_realm(target)),
            },
            // The same table `Promise.prototype` answers from, so an instance and the
            // prototype can never resolve a name differently.
            Value::Promise(_) => match promise_method(name) {
                Value::Undefined => {
                    inherited_object_property(&self.intrinsics, name, value_realm(target))
                }
                method => method,
            },
            // A generator object is its own iterator, which is what lets one be used directly in
            // `for-of` and in a spread.
            // An async generator's `next` hands back a promise of the step, and it answers to
            // `@@asyncIterator` rather than `@@iterator`: a `for-of` over one would get result
            // objects that are promises, which is exactly the mistake `for await` exists to
            // avoid, so it is left to fail rather than half-work.
            Value::Generator(state) if is_async_generator(state) => match name {
                "next" => Value::Callback(CallbackMethod::AsyncGeneratorNext, RealmId::PRIMARY),
                // `return` and `throw` answer with promises for the same reason `next` does: the
                // body may `await` inside a `finally`, or inside the `catch` that receives the
                // thrown value, and none of that can finish inside this call.
                "return" => Value::Callback(CallbackMethod::AsyncGeneratorReturn, RealmId::PRIMARY),
                "throw" => Value::Callback(CallbackMethod::AsyncGeneratorThrow, RealmId::PRIMARY),
                key if key == WellKnown::AsyncIterator.property_key() => {
                    Value::Native(Native::IteratorSelf, RealmId::PRIMARY)
                }
                // The tag, which `%AsyncGeneratorPrototype%` owns and a generator reaches
                // through its chain: `Object.prototype.toString` reads it, and without it every
                // async generator reported `[object Object]`.
                other => generator_prototype_method(true, other),
            },
            // A generator object is its own iterator, which is what lets one be used directly in
            // `for-of` and in a spread.
            Value::Generator(_) => match name {
                "next" => Value::Callback(CallbackMethod::GeneratorNext, RealmId::PRIMARY),
                "return" => Value::Callback(CallbackMethod::GeneratorReturn, RealmId::PRIMARY),
                // `throw` raises at the suspension point, so the generator's own `try` sees it.
                // Without it a delegating `yield*` had nothing to forward a thrown completion to.
                "throw" => Value::Callback(CallbackMethod::GeneratorThrow, RealmId::PRIMARY),
                // Everything else comes from `%IteratorPrototype%`, which a generator object
                // reaches through two links of its chain -- and which is the only reason
                // `g().map(f)` is a thing a program can write.
                // The tag first, from `%GeneratorPrototype%`; everything after it comes from
                // `%IteratorPrototype%`, which a generator reaches through two links of its
                // chain -- and which is the only reason `g().map(f)` is a thing a program can
                // write.
                other => match generator_prototype_method(false, other) {
                    Value::Undefined => {
                        iterator_prototype_property(&self.intrinsics, other, value_realm(target))
                    }
                    found => found,
                },
            },
            _ => Value::Undefined,
        }
    }
}
