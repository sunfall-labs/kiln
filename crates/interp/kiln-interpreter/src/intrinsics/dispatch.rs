use super::super::*;

fn is_frozen_array(value: &Value) -> bool {
    matches!(value, Value::Array(items) if items.properties.borrow().is_frozen())
        || matches!(value, Value::Shared(facade) if facade.is_array())
}

/// The code unit at an already-integral position, or `None` when it is outside the string.
///
/// Reads the stored representation in place, so it is O(1) for Latin1 and UTF-16 strings alike.
/// The position-taking string methods used to copy every unit into a fresh vector first, which
/// made `for (i < s.length) s.charCodeAt(i)` quadratic in the string's length.
fn unit_in_range(text: &StringUnits, index: f64) -> Option<u16> {
    if index >= 0.0 && index < text.unit_len() as f64 {
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let at = index as usize;
        text.unit_at(at)
    } else {
        None
    }
}

pub(crate) fn call_native(
    intrinsics: &Intrinsics,
    native: Native,
    receiver: &Value,
    arguments: &[Value],
) -> Result<Value, ExecutionError> {
    // A wrapper is opened once, here, rather than in each of the methods that has to see through
    // one. Which methods those are is `receiver_for`'s business.
    if let Some(boxed) = receiver_for(native, receiver) {
        return call_native(intrinsics, native, &boxed, arguments);
    }
    // Checked once rather than in each of the four, and before any of them has taken a borrow.
    if native.changes_array_length() && is_frozen_array(receiver) {
        return Err(ExecutionError::FrozenArray);
    }
    Ok(match native {
        // Routed through `Vm::temporal_dispatch` before it can reach here: every `Temporal`
        // method may read an options bag, and a property read is observable and needs the
        // machine. Arriving here would mean the two dispatch paths disagree.
        // Making a realm is `Vm::create_realm`'s: it has to extend the machine's realm tables,
        // and this path has no machine. Arriving here would mean the two dispatch paths disagree.
        Native::HostCreateRealm | Native::HostEvalScript | Native::HostAgent(_) => {
            return Err(ExecutionError::TypeErrorMessage {
                message: "a $262 host facility reached the machine-free dispatch path".to_owned(),
            });
        }
        // Every one of the three needs the machine for the same reason `$262.createRealm` does:
        // making a realm extends the machine's realm tables, and `evaluate` compiles and runs
        // source in one. Routed through `Vm::native_dispatch` before they can reach here;
        // arriving here would mean the two dispatch paths disagree.
        Native::ShadowRealmConstructor
        | Native::ShadowRealmEvaluate
        | Native::ShadowRealmImportValue => {
            return Err(ExecutionError::TypeErrorMessage {
                message: "ShadowRealm reached the machine-free dispatch path".to_owned(),
            });
        }
        Native::Temporal(_) => {
            return Err(ExecutionError::TemporalFailure {
                kind: ErrorKind::Type,
                message: "Temporal reached the machine-free dispatch path".to_owned(),
            });
        }
        // Unreachable: `of` builds through its `this`        // Routed the same way and unreachable for the same reason: `of` builds through its `this`
        // and so needs the machine that can run a constructor.
        Native::TypedArrayOf(_) => {
            return Err(ExecutionError::TypeErrorMessage {
                message: "%TypedArray%.of was dispatched without its constructor".to_owned(),
            });
        }
        // Routed the same way, and unreachable for the same reason: an `Intl` option read is
        // observable, so it cannot happen where there is no machine to observe it with.
        Native::Intl(_) => {
            return Err(ExecutionError::IntlFailure {
                kind: ErrorKind::Type,
                message: "Intl reached the machine-free dispatch path".to_owned(),
            });
        }
        // A proxy of an array *is* an array: `isArray` looks through as many proxies as it
        // takes, which is what lets a library wrap one and still branch on the shape.
        Native::ArrayIsArray => {
            let mut current = arguments.first().unwrap_or(&Value::Undefined);
            while let Value::Proxy(proxy) = current {
                if proxy.revoked.get() {
                    return Err(ExecutionError::RevokedProxy);
                }
                current = &proxy.target;
            }
            Value::Boolean(matches!(
                current,
                Value::Array(_) | Value::Native(Native::PrototypeOf(PrototypeTable::Array), _)
            ) || matches!(current, Value::Shared(facade) if facade.is_array()))
        }
        Native::BigIntConstructor => {
            let Some(value) = arguments.first() else {
                return Ok(Value::BigInt(Rc::new(num_bigint::BigInt::from(0))));
            };
            match value {
                Value::BigInt(_) => value.clone(),
                Value::String(text) => match text
                    .to_scalar_string()
                    .ok()
                    .and_then(|text| string_to_big_int(&text))
                {
                    Some(parsed) => Value::BigInt(Rc::new(parsed)),
                    // A string that is not an integer is a SyntaxError, unlike `Number("x")`
                    // which is NaN -- a BigInt has no NaN to be.
                    None => return Err(ExecutionError::InvalidJson),
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
            // `thisBigIntValue`, with the same refusal: `undefined` for a receiver that is not
            // one reads as an answer rather than as the TypeError the language gives.
            let held;
            let value = match receiver {
                Value::BigInt(value) => value,
                other => match wrapped_primitive(other) {
                    Some(Value::BigInt(inner)) => {
                        held = inner;
                        &held
                    }
                    _ => {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: format!(
                                "BigInt.prototype.toString called on {}",
                                receiver.to_display_string()
                            ),
                        });
                    }
                },
            };
            match arguments.first().map(Value::as_number) {
                None | Some(10.0) => Value::String(value.to_string().into()),
                Some(radix) if (2.0..=36.0).contains(&radix) => {
                    Value::String(value.to_str_radix(radix as u32).into())
                }
                _ => return Err(ExecutionError::RadixOutOfRange),
            }
        }
        // Each of the three below is `ToObject(this)` before anything else. Answering `false`
        // for `undefined` reads, to a feature-detect, exactly like "the property is not there".
        Native::ObjectHasOwnProperty
        | Native::ObjectPropertyIsEnumerable
        | Native::ObjectIsPrototypeOf
            if matches!(receiver, Value::Undefined | Value::Null) =>
        {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Cannot convert undefined or null to object".to_owned(),
            });
        }
        Native::ObjectHasOwnProperty => {
            // All receivers use the same own-descriptor authority as Object.hasOwn. Falling
            // back to HasProperty for an exotic receiver incorrectly includes its prototype.
            let value = arguments.first().unwrap_or(&Value::Undefined);
            let key = value.property_key_units_checked()?;
            Value::Boolean(
                own_attributes_key(intrinsics, receiver, PropertyKeyRef::from_value(value, &key))
                    .is_some(),
            )
        }
        // Walked with [`prototype_of`], the same function `Object.getPrototypeOf` and
        // `instanceof` use. Reading the stored link directly is not the chain: an intrinsic kind
        // -- a date, an array, a typed array -- has no stored link and still has a prototype, so
        // the private walk answered false for every one of them.
        Native::ObjectIsPrototypeOf => {
            let mut current = prototype_of(arguments.first().unwrap_or(&Value::Undefined));
            let mut answer = Value::Boolean(false);
            for _ in 0..MAX_PROTOTYPE_DEPTH {
                if matches!(current, Value::Null) {
                    break;
                }
                if strict_equal(&current, receiver) {
                    answer = Value::Boolean(true);
                    break;
                }
                current = prototype_of(&current);
            }
            answer
        }
        // Own *and* enumerable, which is the pair that separates this from `hasOwnProperty`:
        // `new Error("x").propertyIsEnumerable("message")` is false and `hasOwnProperty` is true.
        Native::ObjectPropertyIsEnumerable => {
            let value = arguments.first().unwrap_or(&Value::Undefined);
            let key = value.property_key_units_checked()?;
            Value::Boolean(
                own_attributes_key(intrinsics, receiver, PropertyKeyRef::from_value(value, &key))
                    .is_some_and(|attributes| attributes.enumerable),
            )
        }
        // `Object.prototype.toString.call(x)`, the oldest type test there is.
        // The tag reports the *box*, not what is in it, which is why this is one of the few
        // built-ins `receiver_for` deliberately leaves a wrapper closed for.
        // Not `[object X]`: this is the *tag*, which `Object.prototype.toString` then wraps.
        // It answered the wrapped form, and answered it for every receiver -- so the getter that
        // serializers use as a typed-array brand check said `"[object Object]"` about a plain
        // object instead of `undefined`, which is true of nothing.
        Native::TypedArrayToStringTag => match wrapped_primitive(receiver)
            .unwrap_or_else(|| receiver.clone())
        {
            Value::TypedArray(view) => Value::String(view.kind.name().to_owned().into()),
            _ => Value::Undefined,
        },
        Native::ObjectToStringTag => Value::String(format!(
            "[object {}]",
            match &wrapped_primitive(receiver).unwrap_or_else(|| receiver.clone()) {
                Value::Undefined => "Undefined",
                Value::Null => "Null",
                Value::Array(_) | Value::Native(Native::PrototypeOf(PrototypeTable::Array), _) => "Array",
                Value::Shared(facade) if facade.is_array() => "Array",
                // The `arguments` object of a call. Ordinary in every other respect, so the
                // brand is the only thing that separates it from the object it otherwise is.
                Value::Object(object) if object.borrow().arguments_brand => "Arguments",
                // `Object.prototype.toString.call(new TypeError("x"))` is `[object Error]` for
                // every error kind, the brand being the [[ErrorData]] slot rather than the
                // constructor. Answering `[object Object]` broke the oldest cross-realm type
                // test there is -- `toString.call(e) === "[object Error]"` is how a library
                // recognises an error it did not create, and `instanceof` cannot do it.
                Value::Object(object) if object.borrow().error_brand => "Error",
                Value::Boolean(_) => "Boolean",
                Value::Number(_) => "Number",
                Value::String(_) => "String",
                Value::Date(_) => "Date",
                Value::RegExp(_) => "RegExp",
                Value::Promise(_) => "Promise",
                Value::TypedArray(view) => view.kind.name(),
                Value::ArrayBuffer(buffer) => {
                    if buffer.shared {
                        "SharedArrayBuffer"
                    } else {
                        "ArrayBuffer"
                    }
                }
                Value::DataView(_) => "DataView",
                // Not a builtinTag: each of these reaches its tag through the
                // `Symbol.toStringTag` its prototype carries, and lands in the same string.
                Value::BigInt(_) => "BigInt",
                Value::Symbol(_) => "Symbol",
                Value::Generator(_) => "Generator",
                // Every `Intl` prototype carries a `Symbol.toStringTag` of its own, so the tag is
                // the qualified name rather than the bare `Object` a shape test would give.
                Value::Intl(built) => built.service().tag(),
                // A namespace and a method table are objects that happen to be spelled as
                // natives here; only the callable ones report as functions.
                Value::Native(native, _) if namespace_name(native).is_some() => "Object",
                Value::Native(Native::PrototypeOf(_), _) => "Object",
                // Each `Intl` prototype carries a `Symbol.toStringTag` of its own, so it reports
                // the qualified name rather than the bare `Object` a shape test would give.
                Value::Native(Native::Intl(intl::Op::Prototype(service)), _) => service.tag(),
                Value::Function(_)
                | Value::Native(_, _)
                | Value::Callback(_, _)
                | Value::Settler(..) | Value::Capability(..) | Value::Element(_)
                | Value::Bound(_)
                | Value::Wrapped(_) => "Function",
                // `IsArray` looks through a proxy, and this tag is built on `IsArray` rather
                // than on the shape of the receiver -- so a proxy of an array reports `Array`,
                // which is the whole reason a library may wrap one and still be recognised.
                other if matches!(
                    unwrap_proxy(other),
                    Value::Array(_) | Value::Native(Native::PrototypeOf(PrototypeTable::Array), _)
                ) || matches!(unwrap_proxy(other), Value::Shared(facade) if facade.is_array()) => "Array",
                _ => "Object",
            }
        ).into()),
        // `Object.prototype.valueOf` is `ToObject(this)`, which for an object is the identity
        // that makes `ToPrimitive` fall through to `toString`, and for a primitive is the box.
        // A wrapper reaches here closed, which is what makes `valueOf` on one answer the object
        // rather than what is in it.
        Native::ObjectValueOf => match to_object(receiver) {
            Some(object) => object,
            None => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Cannot convert undefined or null to object".to_owned(),
                });
            }
        },
        // `thisSymbolValue`: a symbol, or a symbol in a wrapper, and nothing else. Answering the
        // display form of whatever it was called on meant
        // `Symbol.prototype.toString.call({})` read back "[object Object]" rather than refusing,
        // which a program probing for a symbol cannot tell from a real answer.
        Native::PrimitiveToString => {
            let symbol = match receiver {
                Value::Symbol(_) => receiver.clone(),
                other => match wrapped_primitive(other) {
                    Some(inner @ Value::Symbol(_)) => inner,
                    _ => {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: format!(
                                "Symbol.prototype.toString called on {}",
                                receiver.to_display_string()
                            ),
                        });
                    }
                },
            };
            Value::String(symbol.to_string_units_checked()?)
        }
        // `Function.prototype.toString` is one of the few built-ins that checks what it was
        // called on: the specification throws for a receiver that is not callable rather than
        // describing it. `Function.prototype.toString.call({})` is a TypeError, and answering
        // `"[object Object]"` let a program that probes with it read an object as a function.
        Native::FunctionToString => {
            if !is_callable(receiver) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!(
                        "Function.prototype.toString called on {}",
                        receiver.to_display_string()
                    ),
                });
            }
            match receiver {
                Value::Function(closure) => Value::String(closure.source_units()),
                _ => Value::String(receiver.to_display_string().into()),
            }
        }
        // Both build a program from source, which needs the machine: a compilation has to be
        // entered as a frame and run. `Vm::dispatch_native` intercepts them before this, and
        // every call of a built-in goes through it -- so reaching here means a caller found a
        // route around the machine, and refusing says so rather than answering `undefined`.
        Native::FunctionConstructor
        | Native::AsyncFunctionConstructor
        | Native::GeneratorFunctionConstructor
        | Native::AsyncGeneratorFunctionConstructor
        | Native::Eval => {
            return Err(ExecutionError::EvalUnsupported {
                message: "source compilation reached without a machine to run it".to_owned(),
            });
        }
        // `Proxy()` without `new` is a TypeError in the specification; there is nothing useful
        // to return either way.
        Native::ProxyConstructor => Value::Undefined,
        Native::ProxyRevocable => {
            let target = arguments.first().cloned().unwrap_or(Value::Undefined);
            let handler = arguments.get(1).cloned().unwrap_or(Value::Undefined);
            if !is_an_object(&target) || !is_an_object(&handler) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Cannot create proxy with a non-object as target or handler"
                        .to_owned(),
                });
            }
            let proxy_data = Rc::new(ProxyData {
                target,
                handler,
                private_elements: RefCell::new(ObjectData::default()),
                revoked: std::cell::Cell::new(false),
            });
            crate::gc_census::register_proxy(&proxy_data);
            let proxy = Value::Proxy(proxy_data);
            let mut pair = ObjectData::default();
            // The revoke function carries the proxy as its receiver, which is what a bound
            // native is for: there is no other way to give a built-in a captured value.
            pair.set(
                "revoke",
                Value::Bound(Rc::new(BoundFunction {
                    target: Value::Native(Native::ProxyRevoke, RealmId::PRIMARY),
                    this_value: proxy.clone(),
                    arguments: Vec::new(),
                    properties: RefCell::new(ObjectData::default()),
                })),
            );
            pair.set("proxy", proxy);
            Value::Object(tracked_object(pair))
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
        Native::ObjectIs => Value::Boolean(same_value(
            arguments.first().unwrap_or(&Value::Undefined),
            arguments.get(1).unwrap_or(&Value::Undefined),
        )),
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
        Native::ArrayOf => Value::Array(ArrayData::new(arguments.to_vec())),
        Native::StringFromCharCode => Value::String(StringUnits::from_units(
            &arguments
                .iter()
                .map(|value| value.as_number() as u16)
                .collect::<Vec<_>>(),
        )),
        // Each argument is a whole *code point*, not a UTF-16 unit, so an astral one becomes the
        // surrogate pair that spells it. This is what `harness/regExpUtils.js` builds its test
        // strings with, so its absence took every property-escape test with it.
        Native::StringFromCodePoint => {
            let mut units = Vec::with_capacity(arguments.len());
            for argument in arguments {
                let number = argument.as_number();
                // One comparison for the specification's three refusals: `ToIntegerOrInfinity`
                // differing from the number itself rejects a fraction, a `NaN` and an infinity,
                // and the range check rejects anything outside Unicode.
                if number.fract() != 0.0 || !(0.0..=1_114_111.0).contains(&number) {
                    return Err(ExecutionError::InvalidCodePoint {
                        value: argument.to_display_string(),
                    });
                }
                let code_point = number as u32;
                if let Ok(unit) = u16::try_from(code_point) {
                    units.push(unit);
                } else {
                    let offset = code_point - 0x1_0000;
                    units.push(0xD800 + (offset >> 10) as u16);
                    units.push(0xDC00 + (offset & 0x3FF) as u16);
                }
            }
            Value::String(StringUnits::from_units(&units))
        }
        // `SetIntegrityLevel` defines every own property afresh, and an integer-indexed one
        // refuses every flag it would ask for: an element cannot be made non-writable or
        // non-configurable, so a view with anything in it cannot be frozen or sealed at all.
        // Answering `undefined` and quietly doing less made `Object.isFrozen` disagree with the
        // call right above it.
        Native::ObjectFreeze
            if matches!(arguments.first(), Some(Value::TypedArray(view)) if view.len() > 0) =>
        {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Cannot freeze array buffer views with elements".to_owned(),
            });
        }
        Native::ObjectFreeze => {
            match arguments.first().unwrap_or(&Value::Undefined) {
                Value::Object(object) => object.borrow_mut().freeze(),
                // An array's *elements* are not properties of its property store, so freezing
                // has to reach them separately. Without this, `Object.freeze(list)` looked like
                // it worked and changed nothing at all.
                Value::Array(items) => {
                    // A frozen array's `length` is not writable either, which is what
                    // `Object.isFrozen` asks about and what stops a `push` afterwards.
                    items.length_writable.set(false);
                    items.properties.borrow_mut().freeze();
                }
                Value::Function(closure) => closure.properties.borrow_mut().freeze(),
                Value::Bound(bound) => bound.properties.borrow_mut().freeze(),
                // An exotic object's named properties freeze like any other. Its elements do
                // not go with them: an integer-indexed property cannot be made non-writable,
                // which is why `Object.freeze` on a non-empty typed array is a `TypeError` in
                // the specification rather than a freeze that silently does less than it says.
                target if exotic_properties(target).is_some() => {
                    if let Some(store) = exotic_properties(target) {
                        store.borrow_mut().freeze();
                    }
                }
                _ => {}
            }
            // Returns its argument, which is what makes `const o = Object.freeze({…})` read well.
            arguments.first().cloned().unwrap_or(Value::Undefined)
        }
        Native::ObjectIsFrozen => Value::Boolean(match arguments.first() {
            Some(Value::Shared(_)) => true,
            Some(Value::Object(object)) => object.borrow().is_frozen(),
            Some(Value::Array(items)) => items.properties.borrow().is_frozen(),
            Some(Value::Function(closure)) => closure.properties.borrow().is_frozen(),
            Some(Value::Bound(bound)) => bound.properties.borrow().is_frozen(),
            // A view with any element in it is never frozen: an integer-indexed property is
            // writable and stays writable, so there is a property that can still change.
            Some(Value::TypedArray(view)) => {
                view.len() == 0 && view.properties.borrow().is_frozen()
            }
            Some(target) if exotic_properties(target).is_some() => exotic_properties(target)
                .is_some_and(|store| store.borrow().is_frozen()),
            // A built-in -- `Object`, `Math`, `JSON`, `Error.prototype` -- is an object whose
            // methods are writable and configurable, so it is not frozen and never was. It fell
            // into the primitive arm below and answered `true`, which also contradicted
            // `isSealed` answering `false` for the very same value.
            Some(target) if is_object_type(target) => false,
            // A primitive cannot be changed at all, so it is frozen by definition.
            _ => true,
        }),
        Native::ObjectHasOwn => Value::Boolean(match arguments.first() {
            Some(target) => {
                let value = arguments.get(1).unwrap_or(&Value::Undefined);
                let key = value.property_key_units_checked()?;
                own_attributes_key(intrinsics, target, PropertyKeyRef::from_value(value, &key))
                    .is_some()
            }
            None => false,
        }),
        Native::ObjectCreate => {
            let mut data = ObjectData::default();
            match arguments.first() {
                // `Object.create(null)` records the absence, which is not the same as recording
                // nothing: an object with no link at all inherits `Object.prototype`.
                Some(Value::Null) => data.set_prototype(Value::Null),
                Some(Value::Undefined) | None => {}
                Some(prototype) => data.set_prototype(prototype.clone()),
            }
            Value::Object(tracked_object(data))
        }
        // `Object.seal` is the shared bootstrap's (`kiln-builtins/src/object_reflect.ts`), over
        // this and `defineProperty`.
        Native::ObjectPreventExtensions => {
            let target = arguments.first().cloned().unwrap_or(Value::Undefined);
            let close = |data: &mut ObjectData| data.prevent_extensions();
            match &target {
                Value::Object(object) => close(&mut object.borrow_mut()),
                Value::Function(closure) => close(&mut closure.properties.borrow_mut()),
                Value::Bound(bound) => close(&mut bound.properties.borrow_mut()),
                Value::Array(items) => {
                    close(&mut items.properties.borrow_mut());
                }
                // A built-in has no store of its own; the realm keeps one for it, and that is
                // where the closure has to be recorded or the next `isExtensible` undoes it.
                Value::Native(_, _) | Value::Callback(_, _) | Value::Settler(..) | Value::Capability(..) | Value::Element(_) => {
                    intrinsics.close(&target, false);
                }
                // An exotic object closes its named store. Its elements are not slots of that
                // store and keep the flags the specification fixes for them, which is why a
                // sealed view with elements still reports itself unsealed.
                other if exotic_properties(other).is_some() => {
                    if let Some(store) = exotic_properties(other) {
                        close(&mut store.borrow_mut());
                    }
                }
                _ => {}
            }
            target
        }
        Native::ObjectIsExtensible => Value::Boolean(match arguments.first() {
            // A primitive has no shape to close, so nothing can be added to it.
            None | Some(Value::Undefined | Value::Null) => false,
            Some(target) if !is_reflect_target(target) => false,
            Some(target) => is_extensible_target(intrinsics, target),
        }),
        // Symbol keys are stored as reserved strings, and a string cannot be turned back into
        // the symbol it came from -- so this reports none rather than inventing any.
        // The machine answers this one: recovering a symbol from the reserved key it is stored
        // under needs the realm's registry, which a free function has no way to reach.
        Native::ObjectGetOwnPropertySymbols => {
            unreachable!("`Object.getOwnPropertySymbols` is dispatched by the interpreter")
        }
        Native::NodeAsyncLocalStorage
        | Native::NodeAsyncLocalStorageRun
        | Native::NodeAsyncLocalStorageGetStore
        | Native::NodeAsyncLocalStorageEnterWith
        | Native::NodeAsyncLocalStorageExit
        | Native::NodeAsyncLocalStorageDisable => {
            unreachable!("`AsyncLocalStorage` is dispatched by the interpreter")
        }
        Native::ObjectGetOwnPropertyDescriptor => {
            let target = to_object(arguments.first().unwrap_or(&Value::Undefined))
                .ok_or_else(|| ExecutionError::TypeErrorMessage {
                    message: "Cannot convert undefined or null to object".to_owned(),
                })?;
            let value = arguments.get(1).unwrap_or(&Value::Undefined);
            let key = value.property_key_units_checked()?;
            own_descriptor_key(intrinsics, &target, PropertyKeyRef::from_value(value, &key))
                .unwrap_or(Value::Undefined)
        }
        Native::ObjectGetOwnPropertyDescriptors => {
            let target = to_object(arguments.first().unwrap_or(&Value::Undefined))
                .ok_or_else(|| ExecutionError::TypeErrorMessage {
                    message: "Cannot convert undefined or null to object".to_owned(),
                })?;
            let mut data = ObjectData::default();
            for key in own_property_keys_units(intrinsics, &target) {
                if let Some(descriptor) =
                    own_descriptor_key(intrinsics, &target, PropertyKeyRef::Units(&key))
                {
                    data.set_units(&key, descriptor);
                }
            }
            for key in own_symbol_keys_units(intrinsics, &target) {
                if let Some(descriptor) =
                    own_descriptor_key(intrinsics, &target, PropertyKeyRef::SymbolUnits(&key))
                {
                    data.set_key(PropertyKeyRef::SymbolUnits(&key), descriptor);
                }
            }
            Value::Object(tracked_object(data))
        }
        Native::Reflect => Value::Undefined,
        Native::ObjectOwnNames => {
            // Every own property, enumerated or not. A function's own properties are included
            // for a reason that took a while to find: the CommonJS interop every bundler emits
            // copies a module's exports with this, and a module whose exports *are* a function
            // -- lodash, and most of the older ecosystem -- copied nothing without it.
            Value::Array(ArrayData::new(
                own_property_names_units(intrinsics, arguments.first().unwrap_or(&Value::Undefined))
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ))
        }
        Native::ObjectKeys => {
            let keys = match arguments.first() {
                // A function's own properties are the ones assigned to it. This matters more
                // than it looks: the CommonJS interop every bundler emits copies a module's
                // exports with `getOwnPropertyNames`, and a module whose exports *are* a
                // function -- lodash, and most of the older ecosystem -- copied nothing.
                //
                // `entries` rather than `keys`, for the reason the object arm gives: a class's
                // static methods are own properties of the constructor and are not enumerated,
                // so `Object.keys(class { static s() {} })` is empty.
                Some(Value::Function(closure)) => closure
                    .properties
                    .borrow()
                    .string_entries()
                    .map(|(key, _)| Value::String(key.clone()))
                    .collect(),
                Some(Value::Bound(bound)) => bound
                    .properties
                    .borrow()
                    .string_entries()
                    .map(|(key, _)| Value::String(key.clone()))
                    .collect(),
                // `entries` rather than `keys`: this reports what is *enumerated*, where
                // `getOwnPropertyNames` reports everything. An error's own properties are the
                // case where the two differ.
                Some(target @ Value::Object(object)) => string_wrapper_entries_units(target)
                    .into_iter()
                    .map(|(key, _)| Value::String(key))
                    .chain(
                        object
                            .borrow()
                            .string_entries()
                            .map(|(key, _)| Value::String(key.clone())),
                    )
                    .collect(),
                // A built-in's own enumerated properties are the ones a program added -- the
                // tables' own are non-enumerable, exactly as the specification defines them --
                // with the one exception `own_properties` knows about: a Node builtin module's
                // exports *are* enumerable. Asked of that function rather than of the realm
                // store directly, so `Object.keys`, `Object.entries` and a spread cannot give
                // three different answers about the same namespace.
                Some(target @ (Value::Native(_, _) | Value::Callback(_, _))) => {
                    own_properties_units(intrinsics, target)
                        .into_iter()
                        .map(|(key, _)| Value::String(key))
                        .collect()
                }
                // `Object.keys([1,2])` is ["0","1"]: array indices are string keys too, and a
                // hole has no key at all.
                // The elements, and then whatever the program hung on the array by name --
                // `length` is not among them, because it is not enumerable. Stopping at the
                // elements made `Object.keys` disagree with the `for-in` beside it about the
                // same array.
                Some(target @ Value::Array(_)) => own_property_keys_units(intrinsics, target)
                    .into_iter()
                    .filter(|key| {
                        own_attributes_key(intrinsics, target, PropertyKeyRef::Units(key))
                            .is_some_and(|attributes| attributes.enumerable)
                    })
                    .map(Value::String)
                    .collect(),
                Some(target @ Value::Shared(_)) => own_properties_units(intrinsics, target)
                    .into_iter()
                    .map(|(key, _)| Value::String(key))
                    .collect(),
                // The elements, and then whatever the program hung on the view by name: an
                // exotic object enumerates both, in the order its own-key walk lists them.
                Some(Value::TypedArray(view)) => (0..view.len())
                    .map(|index| Value::String(index.to_string().into()))
                    .chain(
                        view.properties
                            .borrow()
                            .string_entries()
                            .map(|(key, _)| Value::String(key.clone())),
                    )
                    .collect(),
                Some(target) if exotic_properties(target).is_some() => exotic_properties(target)
                    .map(|store| {
                        store
                            .borrow()
                            .string_entries()
                            .map(|(key, _)| Value::String(key.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            Value::Array(ArrayData::new(keys))
        }
        // Converting an object means calling its `toString`, which needs the machine.
        Native::StringConstructor => {
            unreachable!("`String` is dispatched by the interpreter")
        }
        Native::ErrorToString => Value::String(error_to_string(receiver).into()),
        Native::RegExpToString => Value::String(receiver.to_string_units_checked()?),
        // Reading `{ maxByteLength }` off the options bag is an ordinary property read, which
        // may run a getter, so the machine has to be the one to do it.
        Native::ArrayBufferConstructor | Native::SharedArrayBufferConstructor => {
            unreachable!("`ArrayBuffer` is dispatched by the interpreter")
        }
        // Reading `{ alphabet }` and `{ lastChunkHandling }` off an options bag is an ordinary
        // property read, which may run a getter, so the machine has to be the one to do it.
        Native::Uint8Text(_) => {
            unreachable!("a Uint8Array text conversion is dispatched by the interpreter")
        }
        // Every `DataView` entry point coerces an argument, and coercing one runs a method.
        Native::DataViewConstructor
        | Native::DataViewGet(_)
        | Native::DataViewSet(_)
        | Native::DataViewGetBig { .. }
        | Native::DataViewSetBig { .. } => {
            unreachable!("`DataView` is dispatched by the interpreter")
        }
        // Every iterator method drives another iterator, which means calling code.
        Native::IteratorLazy(_)
        | Native::IteratorTerminal(_)
        | Native::IteratorHelperNext(_)
        | Native::IteratorHelperReturn => {
            unreachable!("an iterator helper is dispatched by the interpreter")
        }
        // `Iterator` is abstract: it exists to be the thing every iterator answers to and to
        // hold the prototype the helpers live on, and calling or constructing it is a TypeError
        // rather than an iterator over nothing.
        Native::IteratorConstructor => {
            return Err(ExecutionError::NotConstructable {
                instruction: 0,
                value: "abstract iterator intrinsic".into(),
            });
        }
        // `slice` *copies*: the result is a fixed-length buffer of its own, so a later resize of
        // the original -- or a write through a view onto it -- is not visible through the copy.
        // A *view*, not a buffer: `isView` asks whether the value reads bytes through one, which
        // the buffer itself does not.
        Native::ArrayBufferIsView => Value::Boolean(matches!(
            arguments.first(),
            Some(Value::TypedArray(_) | Value::DataView(_))
        )),
        // `__kilnArrayBufferDetach(buffer, length, preserveResizability)`, the one primitive a
        // transfer needs (ADR 0065, representation): the bytes are *moved*, not copied, into the
        // buffer that comes out, and the one they came from holds nothing afterwards. Every
        // check a program can observe -- the receiver, the length, the ceiling -- is made above
        // it, in `kiln-builtins/src/arraybuffer.ts` and `structured_clone.ts`; what is left here
        // only keeps a caller that skipped them from corrupting a buffer.
        // `__kilnCloneData(value)` (ADR 0065, a *measured* primitive): the structured clone of a
        // value made only of plain data, or `undefined` -- "do it the general way" -- for
        // anything else, which the shared clone (`kiln-builtins/src/structured_clone.ts`) then
        // does from the start. The same subset as the native backend's (`runtime/spread.c`).
        Native::CloneDataRaw => {
            // Native mode 1 builds the message wire directly; this adapter leaves it to the
            // TypeScript encoder. Returning undefined is its ordinary guarded fallback.
            if matches!(arguments.get(1), Some(Value::Number(mode)) if *mode == 1.0) {
                Value::Undefined
            } else {
                let mut seen = std::collections::HashMap::new();
                arguments
                    .first()
                    .and_then(|value| clone_plain_data(value, &mut seen, 0))
                    .unwrap_or(Value::Undefined)
            }
        }
        Native::SharedIsRaw => Value::Boolean(matches!(arguments.first(), Some(Value::Shared(_)))),
        Native::ShareRaw => unreachable!("share needs the VM's realm facade cache"),
        // `__kilnArrayBufferResize(buffer, length)`: the storage half of `resize` and `grow`. The
        // receiver, the conversion of the length, detachment and the ceiling are all checked
        // above it in `kiln-builtins/src/arraybuffer.ts`; the buffer's own checks here only keep a
        // caller that skipped them from corrupting it.
        // `__kilnTypedArrayOutOfBounds(view)`: `IsTypedArrayOutOfBounds`, for a view the shared
        // TypeScript has already brand-checked. Anything else is not a view with a window.
        Native::TypedArrayOutOfBoundsRaw => match arguments.first() {
            Some(Value::TypedArray(view)) => Value::Boolean(view.out_of_bounds()),
            _ => Value::Boolean(false),
        },
        Native::IsConstructorRaw => Value::Boolean(arguments.first().is_some_and(is_constructor)),
        Native::IsOtherRealmArrayConstructorRaw => {
            return Err(ExecutionError::TypeErrorMessage {
                message: "GetFunctionRealm reached the machine-free dispatch path".to_owned(),
            });
        }
        Native::ArrayNextPresentIndexRaw => {
            let object = arguments.first().unwrap_or(&Value::Undefined);
            let cursor = match arguments.get(1) {
                Some(Value::Number(value)) => *value,
                _ => f64::NAN,
            };
            let limit = match arguments.get(2) {
                Some(Value::Number(value)) => *value,
                _ => f64::NAN,
            };
            let reverse = matches!(arguments.get(3), Some(Value::Boolean(true)));
            if !cursor.is_finite() || !limit.is_finite() || cursor < 0.0 {
                Value::Boolean(false)
            } else {
                let cursor = cursor as usize;
                let limit = if reverse {
                    if limit < 0.0 { 0 } else { limit as usize }
                } else if limit < 0.0 {
                    0
                } else {
                    limit as usize
                };
                match next_array_present_index(intrinsics, object, cursor, limit, reverse) {
                    None => Value::Boolean(false),
                    Some(None) => Value::Null,
                Some(Some(index)) => Value::Number(index as f64),
            }
            }
        }
        Native::DataViewStateRaw => Value::Number(match arguments.first() {
            Some(Value::DataView(view)) if view.buffer.immutable => 2.0,
            Some(Value::DataView(_)) => 1.0,
            _ => 0.0,
        }),
        Native::CollectionSizeRaw => {
            let Some(receiver) = arguments.first() else {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "get size: receiver does not have the required internal slot".to_owned(),
                });
            };
            let expected_map = match arguments.get(1) {
                Some(Value::Boolean(kind)) => Some(*kind),
                _ => None,
            };
            if is_weak_collection(receiver) || !collection_state(receiver)
                .is_some_and(|(_, is_map)| Some(is_map) == expected_map) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "get size: receiver does not have the required internal slot".to_owned(),
                });
            }
            let Value::Object(object) = receiver else {
                unreachable!("collection state is stored on an object");
            };
            object
                .borrow()
                .get_own(COLLECTION_SIZE)
                .cloned()
                .unwrap_or(Value::Number(0.0))
        }
        // `__kilnTypedArrayCopy(target, targetIndex, source, sourceIndex, count)` and
        // `__kilnTypedArraySort(view)` (ADR 0065, representation): the shared TypeScript has
        // computed every index; anything else answers `false`, "run the loop".
        Native::TypedArrayCopyRaw => {
            let index = |at: usize| match arguments.get(at) {
                Some(Value::Number(n)) if *n >= 0.0 && n.fract() == 0.0 && *n <= 9007199254740991.0 => {
                    Some(*n as usize)
                }
                _ => None,
            };
            match (arguments.first(), arguments.get(2), index(1), index(3), index(4)) {
                (
                    Some(Value::TypedArray(target)),
                    Some(Value::TypedArray(source)),
                    Some(target_index),
                    Some(source_index),
                    Some(count),
                ) => Value::Boolean(target.copy_elements_from(target_index, source, source_index, count)?),
                _ => Value::Boolean(false),
            }
        }
        Native::TypedArraySortRaw => match arguments.first() {
            Some(Value::TypedArray(view)) => Value::Boolean(view.sort_default()?),
            _ => Value::Boolean(false),
        },
        Native::ArrayBufferResizeRaw => {
            let Some(Value::ArrayBuffer(buffer)) = arguments.first() else {
                return Err(ExecutionError::NotABuffer);
            };
            let length = match arguments.get(1) {
                Some(Value::Number(length)) => bounded_buffer(to_index_number(*length)?, 1)?,
                _ => return Err(ExecutionError::NotABuffer),
            };
            if buffer.shared {
                buffer.grow(length)?;
            } else {
                buffer.resize(length)?;
            }
            Value::Undefined
        }
        Native::ArrayBufferDetachRaw => {
            let Some(Value::ArrayBuffer(buffer)) = arguments.first() else {
                return Err(ExecutionError::NotABuffer);
            };
            // A shared buffer cannot be detached at all: its other holders have no way to learn
            // the bytes went away. Nor can an immutable one: detaching is the largest write.
            if buffer.shared || buffer.detached() {
                return Err(ExecutionError::BufferDetached);
            }
            if buffer.immutable {
                return Err(ExecutionError::BufferImmutable);
            }
            let length = match arguments.get(1) {
                Some(Value::Number(length)) => bounded_buffer(to_index_number(*length)?, 1)?,
                _ => buffer.byte_length(),
            };
            let max_byte_length = if matches!(arguments.get(2), Some(Value::Boolean(true))) {
                buffer.max_byte_length
            } else {
                None
            };
            if let Some(max) = max_byte_length
                && length > max
            {
                return Err(ExecutionError::BufferLengthOutOfRange {
                    length: length as f64,
                });
            }
            let mut bytes = buffer.detach();
            bytes.resize(length, 0);
            Value::ArrayBuffer(Rc::new(BufferData {
                bytes: Bytes::new(false, bytes),
                max_byte_length,
                shared: false,
                detached: std::cell::Cell::new(false),
                immutable: false,
                properties: RefCell::new(ObjectData::default()),
            }))
        }
        Native::ArrayBufferByteLengthRaw => {
            let Some(Value::ArrayBuffer(buffer)) = arguments.first() else {
                return Err(ExecutionError::NotABuffer);
            };
            let shared = arguments.get(1).is_some_and(Value::is_truthy);
            if buffer.shared != shared {
                return Err(ExecutionError::NotABuffer);
            }
            Value::Number(buffer.byte_length() as f64)
        }
        // `ArrayBufferCopyAndDetach(O, newLength, ~immutable~)`: the same copy `transfer`
        // performs, into a buffer nothing can write to afterwards. A ceiling is never carried
        // across -- a length that could still move is exactly what immutability rules out.
        Native::ArrayBufferTransferToImmutable => {
            let Value::ArrayBuffer(buffer) = receiver else {
                return Err(ExecutionError::NotABuffer);
            };
            if buffer.shared || buffer.detached() {
                return Err(ExecutionError::BufferDetached);
            }
            if buffer.immutable {
                return Err(ExecutionError::BufferImmutable);
            }
            let length = match arguments.first() {
                None | Some(Value::Undefined) => buffer.byte_length(),
                requested => bounded_buffer(to_index(requested)?, 1)?,
            };
            let mut bytes = buffer.detach();
            // Growing pads with zeros and shrinking drops the tail, exactly as `transfer` does:
            // the length asked for is the length the result has, whichever side of the source's
            // it falls on.
            bytes.resize(length, 0);
            Value::ArrayBuffer(Rc::new(BufferData {
                bytes: Bytes::new(false, bytes),
                max_byte_length: None,
                shared: false,
                detached: std::cell::Cell::new(false),
                immutable: true,
                properties: RefCell::new(ObjectData::default()),
            }))
        }
        // The copying half of the pair. The source keeps its bytes and is free to be written,
        // resized or detached afterwards -- none of which the result can see, because it holds a
        // copy rather than a window.
        Native::ArrayBufferSliceToImmutable => {
            let Value::ArrayBuffer(buffer) = receiver else {
                return Err(ExecutionError::NotABuffer);
            };
            if buffer.shared {
                return Err(ExecutionError::NotABuffer);
            }
            if buffer.detached() {
                return Err(ExecutionError::BufferDetached);
            }
            let bytes = buffer.bytes.borrow();
            let (start, end) = slice_bounds(arguments, bytes.len());
            Value::ArrayBuffer(Rc::new(BufferData {
                bytes: Bytes::new(false, bytes[start..end].to_vec()),
                max_byte_length: None,
                shared: false,
                detached: std::cell::Cell::new(false),
                immutable: true,
                properties: RefCell::new(ObjectData::default()),
            }))
        }
        Native::ArrayBufferSlice => {
            let Value::ArrayBuffer(buffer) = receiver else {
                return Err(ExecutionError::NotABuffer);
            };
            // There is nothing to copy *from*: a detached buffer has no bytes, and answering an
            // empty slice would tell the program it sliced an empty buffer rather than one it
            // no longer owns.
            if buffer.detached() {
                return Err(ExecutionError::BufferDetached);
            }
            let bytes = buffer.bytes.borrow();
            let (start, end) = slice_bounds(arguments, bytes.len());
            Value::ArrayBuffer(Rc::new(BufferData {
                bytes: Bytes::new(buffer.shared, bytes[start..end].to_vec()),
                max_byte_length: None,
                // A slice of a shared buffer is itself shared, and a slice of an unshared one is
                // not: the specification builds the result with the *receiver's* constructor,
                // and a program that sliced a `SharedArrayBuffer` and got back something it
                // could not hand to another holder would have lost the only property it wanted.
                shared: buffer.shared,
                detached: std::cell::Cell::new(false),
                immutable: false,
                properties: RefCell::new(ObjectData::default()),
            }))
        }
        Native::TypedArrayConstructor(kind) => construct_typed_array(kind, arguments)?,
        // `%TypedArray%` is abstract: it exists to be inherited from, and calling or
        // constructing it is a TypeError rather than a view of some default element type.
        Native::TypedArrayIntrinsic => {
            return Err(ExecutionError::NotConstructable {
                instruction: 0,
                value: "abstract typed array intrinsic".into(),
            });
        }
        // `of` takes its elements as arguments where the constructor takes one list, which is
        // the whole difference: `Uint8Array.of(3)` is one element, `new Uint8Array(3)` is three.
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
            Value::String(out.into())
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
            Value::String(String::from_utf16_lossy(&out).into())
        }
        // The four URI functions differ only in which characters they leave alone. `encodeURI`
        // keeps the ones that structure a URI -- `:/?#[]@!$&'()*+,;=` -- because it is encoding a
        // whole one; `encodeURIComponent` encodes those too, because a component that contains a
        // `/` must not look like a path separator.
        Native::EncodeUri | Native::EncodeUriComponent => {
            let text = arguments
                .first()
                .unwrap_or(&Value::Undefined)
                .to_string_units_checked()?
                .to_scalar_string()
                .map_err(|_| ExecutionError::MalformedUri)?;
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
            Value::String(out.into())
        }
        Native::DecodeUri | Native::DecodeUriComponent => {
            let units = arguments
                .first()
                .cloned()
                .unwrap_or(Value::Undefined)
                .to_string_units_checked()?;
            let is_decode_uri = matches!(native, Native::DecodeUri);
            let len = units.len();
            let mut out: Vec<u16> = Vec::with_capacity(len);
            let mut k = 0;

            let parse_hex_byte = |k: usize| -> Option<u8> {
                let c1 = units.unit_at(k)?;
                let c2 = units.unit_at(k + 1)?;
                if c1 > 0x7F || c2 > 0x7F {
                    return None;
                }
                let d1 = (c1 as u8 as char).to_digit(16)? as u8;
                let d2 = (c2 as u8 as char).to_digit(16)? as u8;
                Some((d1 << 4) | d2)
            };

            while k < len {
                let unit = units.unit_at(k).unwrap_or(0);
                if unit != b'%' as u16 {
                    out.push(unit);
                    k += 1;
                    continue;
                }
                let Some(byte) = parse_hex_byte(k + 1) else {
                    return Err(ExecutionError::MalformedUri);
                };
                k += 3;
                if byte < 0x80 {
                    if is_decode_uri
                        && matches!(
                            byte,
                            b';' | b'/' | b'?' | b':' | b'@' | b'&' | b'=' | b'+' | b'$' | b',' | b'#'
                        )
                    {
                        out.push(b'%' as u16);
                        out.push(units.unit_at(k - 2).unwrap_or(0));
                        out.push(units.unit_at(k - 1).unwrap_or(0));
                    } else {
                        out.push(byte as u16);
                    }
                } else {
                    let (n, min_cp, mut cp) = match byte {
                        0xC2..=0xDF => (2, 0x80u32, (byte & 0x1F) as u32),
                        0xE0..=0xEF => (3, 0x800u32, (byte & 0x0F) as u32),
                        0xF0..=0xF4 => (4, 0x10000u32, (byte & 0x07) as u32),
                        _ => return Err(ExecutionError::MalformedUri),
                    };
                    for _ in 1..n {
                        if k >= len || units.unit_at(k) != Some(b'%' as u16) {
                            return Err(ExecutionError::MalformedUri);
                        }
                        let Some(cont) = parse_hex_byte(k + 1) else {
                            return Err(ExecutionError::MalformedUri);
                        };
                        if (cont & 0xC0) != 0x80 {
                            return Err(ExecutionError::MalformedUri);
                        }
                        k += 3;
                        cp = (cp << 6) | ((cont & 0x3F) as u32);
                    }
                    if cp < min_cp || (0xD800..=0xDFFF).contains(&cp) || cp > 0x10FFFF {
                        return Err(ExecutionError::MalformedUri);
                    }
                    if cp < 0x10000 {
                        out.push(cp as u16);
                    } else {
                        let v = cp - 0x10000;
                        out.push(0xD800 + ((v >> 10) as u16));
                        out.push(0xDC00 + ((v & 0x3FF) as u16));
                    }
                }
            }
            Value::String(StringUnits::from_units(&out))
        }
        Native::ObjectSetPrototypeOf => {
            let target = arguments.first().cloned().unwrap_or(Value::Undefined);
            if let Some(prototype) = arguments.get(1) {
                // A cycle or a sealed target is a `TypeError` here rather than the `false`
                // `Reflect.setPrototypeOf` answers with. Accepting either silently left the
                // object with a chain that no lookup could finish walking.
                if !prototype_may_change(intrinsics, &target, prototype) {
                    return Err(prototype_change_refused());
                }
                // Into the store `prototype_of` already looks in, so the two agree. A function
                // is an object here too: setting its prototype is how a program gives a
                // constructor inherited statics without `extends`.
                record_prototype_in(intrinsics, &target, prototype.clone());
            }
            target
        }
        // `ToObject` first: `Object.getPrototypeOf("")` is `String.prototype`, not null. A
        // primitive has no chain of its own, which is why `prototype_of` answers null for one and
        // why this cannot ask it directly.
        Native::ObjectGetPrototypeOf => {
            let target = arguments.first().unwrap_or(&Value::Undefined);
            let Some(object) = to_object(target) else {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Cannot convert undefined or null to object".to_owned(),
                });
            };
            prototype_of_in(intrinsics, &object)
        }
        // Both read the clock, which lives on the machine and is virtual -- see ADR 0015.
        Native::SymbolFor | Native::SymbolKeyFor => {
            unreachable!("the symbol registry is dispatched by the interpreter")
        }
        // Each of these *reads* a property, which for an accessor is a call, or calls back into
        // the program outright.
        Native::ObjectAssign
        | Native::ObjectToLocaleString
        | Native::ObjectEntries
        | Native::ObjectValues
        | Native::ObjectDefineProperty
        | Native::ReflectDefineProperty
        | Native::ObjectDefineProperties
        | Native::StringRaw
        | Native::CollectionForEach(_)
        | Native::FunctionCall
        | Native::FunctionApply
        | Native::FunctionBind
        | Native::ObjectFromEntries
        | Native::ReflectOwnKeys
        | Native::ReflectGet
        | Native::ReflectSet
        | Native::ReflectApply
        | Native::ReflectConstruct
        | Native::ReflectHas
        | Native::ReflectGetPrototypeOf
        | Native::ReflectSetPrototypeOf
        | Native::ReflectGetOwnPropertyDescriptor
        | Native::ReflectDeleteProperty
        | Native::ReflectPreventExtensions => {
            unreachable!("these built-ins are dispatched by the interpreter")
        }
        Native::DateConstructor | Native::DateNow | Native::PerformanceNow => {
            unreachable!("the clock built-ins are dispatched by the interpreter")
        }
        Native::Date(_) => {
            unreachable!("the date methods are dispatched by the interpreter")
        }
        Native::RegExpAccessor(_) => {
            unreachable!("the regexp accessors are dispatched by the interpreter")
        }
        Native::BuiltinAccessor(_) => {
            unreachable!("the built-in accessors are dispatched by the interpreter")
        }
        Native::SetOperation(_) => {
            unreachable!("the set operations are dispatched by the interpreter")
        }
        Native::Atomics | Native::AtomicsOp(_) => {
            unreachable!("the atomics operations are dispatched by the interpreter")
        }
        Native::CollectionGetOrInsert { .. } => {
            unreachable!("the upsert methods are dispatched by the interpreter")
        }
        Native::ArrayCopy(_) => {
            unreachable!("the copying array methods are dispatched by the interpreter")
        }
        Native::RegExpSymbol(_)
        | Native::JsonRawJson
        | Native::JsonIsRawJson
        | Native::WeakRefConstructor
        | Native::WeakRefDeref
        | Native::DisposableStackConstructor
        | Native::DisposableStackUse
        | Native::DisposableStackAdopt
        | Native::DisposableStackDefer
        | Native::DisposableStackMove
        | Native::DisposableStackDispose
        | Native::AsyncDisposableStackConstructor
        | Native::AsyncDisposableStackUse
        | Native::AsyncDisposableStackAdopt
        | Native::AsyncDisposableStackDefer
        | Native::AsyncDisposableStackMove
        | Native::AsyncDisposableStackDisposeAsync
        | Native::FinalizationRegistryConstructor
        | Native::FinalizationRegistryRegister
        | Native::FinalizationRegistryUnregister
        | Native::RegExpStringIteratorNext
        | Native::IteratorFrom
        | Native::IteratorWrapNext
        | Native::ObjectGroupBy
        | Native::MapGroupBy => {
            unreachable!("these built-ins are dispatched by the interpreter")
        }
        // `%ThrowTypeError%`, in full. The message is the one every engine gives, because a
        // program that reaches this has almost always reached it by accident.
        Native::ThrowTypeError => {
            return Err(ExecutionError::TypeErrorMessage {
                message: "'caller', 'callee', and 'arguments' properties may not be accessed on \
                          strict mode functions or the arguments objects for calls to them"
                    .to_owned(),
            });
        }
        // The two halves of `Object.prototype.__proto__`. A read of the property is answered by
        // the machine without reaching here; these are what a program gets when it takes the
        // accessor apart and calls one directly.
        Native::ProtoGetter => {
            if !is_reflect_target(receiver) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Cannot convert undefined or null to object".to_owned(),
                });
            }
            prototype_of_in(intrinsics, receiver)
        }
        Native::ProtoSetter => {
            if matches!(receiver, Value::Undefined | Value::Null) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Cannot convert undefined or null to object".to_owned(),
                });
            }
            let prototype = arguments.first().cloned().unwrap_or(Value::Undefined);
            // Anything that is neither an object nor null is ignored rather than refused, which
            // is what keeps `x.__proto__ = 1` from being an error in ordinary code.
            if matches!(
                prototype,
                Value::Object(_)
                    | Value::Function(_)
                    | Value::Array(_)
                    | Value::Native(_, _)
                    | Value::Callback(_, _)
                    | Value::Null
            ) {
                // A change the object cannot take is a `TypeError`, exactly as it is through
                // `Object.setPrototypeOf`: this accessor is Annex B's spelling of that
                // operation, not a weaker one.
                if !prototype_may_change(intrinsics, receiver, &prototype) {
                    return Err(prototype_change_refused());
                }
                record_prototype_in(intrinsics, receiver, prototype);
            }
            Value::Undefined
        }
        // `Promise.withResolvers()`: a pending promise and the two functions that settle it, as
        // an object -- which is the shape every "deferred" helper in the wild hand-rolls.
        Native::PromiseWithResolvers => {
            let promise = PromiseState::pending();
            let mut result = ObjectData::default();
            result.set("promise", Value::Promise(Rc::clone(&promise)));
            result.set("resolve", Value::Settler(Rc::clone(&promise), false));
            result.set("reject", Value::Settler(promise, true));
            Value::Object(tracked_object(result))
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
        | Native::RegExpLegacyCaptureRaw
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
        | Native::SetImmediate
        | Native::ClearTimeout
        | Native::ClearInterval
        | Native::ClearImmediate
        | Native::QueueMicrotask
        | Native::ProcessNextTick
        // ...and `Math.random` advances state that lives on the machine.
        | Native::MathRandom
        // Every promise operation settles something, schedules something, or calls back into the
        // program -- so none of them is a pure function of its arguments.
        | Native::PromiseConstructor
        | Native::PromiseResolve
        | Native::PromiseReject
        // `OrdinaryHasInstance` walks a prototype chain, and a link in that chain can be a proxy
        // whose `getPrototypeOf` trap is ordinary code -- so it is dispatched on the machine.
        | Native::FunctionHasInstance
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
            Value::Array(items) => items.borrow_mut().pop().unwrap_or(Value::Undefined),
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
                    Value::Native(Native::Hole, _) => {
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
        Native::DateParse => Value::Number(parse_date(
            &arguments
                .first()
                .map_or_else(String::new, Value::to_display_string),
        )),
        Native::DateUtc => Value::Number(time_clip(date_from_fields(arguments))),
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
                items.borrow_mut().reverse();
                receiver.clone()
            }
            _ => Value::Undefined,
        },
        Native::ArrayShift => match receiver {
            Value::Array(items) => {
                let mut items = items.borrow_mut();
                if items.is_empty() {
                    Value::Undefined
                } else {
                    items.remove(0)
                }
            }
            _ => Value::Undefined,
        },
        Native::ArrayUnshift => match receiver {
            Value::Array(items) => {
                let mut items = items.borrow_mut();
                // Inserted as a block so the arguments keep their order at the front.
                items.splice(0..0, arguments.iter().cloned());
                Value::Number(items.len() as f64)
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
        // `toString` calls the receiver's own `join`, which is program code: it is dispatched by
        // the interpreter, and reaching this arm means the route to it was lost.
        Native::ArrayJoinDefault => {
            unreachable!("`Array.prototype.toString` is dispatched by the interpreter")
        }
        Native::ArraySplice => match receiver {
            Value::Array(items) => {
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
                // The removed elements, which is what makes `splice` usable as a queue read.
                Value::Array(ArrayData::new(removed))
            }
            _ => Value::Undefined,
        },
        Native::ArrayCopyWithin => match receiver {
            Value::Array(items) => {
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
                receiver.clone()
            }
            // A view reads its receiver directly rather than through the dense snapshot the
            // walking methods get -- see the `validates_typed_array` list in `call_value` -- so
            // without an arm of its own it fell to the `undefined` below. That is the silent
            // shape twice over: `view.copyWithin(0, 3)` left every element exactly where it was
            // *and* answered `undefined` instead of the view, so a caller that chained off it
            // failed a step later naming `undefined` rather than the method that was wrong.
            //
            // Writing the view directly is also the only correct spelling. Two views over one
            // buffer are the reason this method exists, and a snapshot taken here would be
            // copied back over writes the overlap had already made.
            Value::TypedArray(view) => {
                let length = view.len();
                let target = relative_index(arguments.first(), length, 0);
                let start = relative_index(arguments.get(1), length, 0);
                let end = relative_index(arguments.get(2), length, length);
                // Read out before writing, for the same overlap reason as the array arm above.
                let taken: Vec<Value> = (start..end)
                    .take(length.saturating_sub(target))
                    .filter_map(|index| view.get(index))
                    .collect();
                for (offset, value) in taken.into_iter().enumerate() {
                    view.set(target + offset, &value)?;
                }
                receiver.clone()
            }
            _ => Value::Undefined,
        },
        Native::ArrayFill => match receiver {
            Value::Array(items) => {
                let value = arguments.first().cloned().unwrap_or(Value::Undefined);
                let length = items.borrow().len();
                let start = relative_index(arguments.get(1), length, 0);
                let end = relative_index(arguments.get(2), length, length);
                for slot in items.borrow_mut().iter_mut().take(end).skip(start) {
                    *slot = value.clone();
                }
                receiver.clone()
            }
            _ => Value::Undefined,
        },
        // `toLocaleString` calls each element's own, which is program code: it is dispatched by
        // the interpreter, and reaching this arm means the route to it was lost.
        Native::ArrayToLocaleString => {
            unreachable!("`Array.prototype.toLocaleString` is dispatched by the interpreter")
        }
        Native::ArrayJoin => {
            let separator = match arguments.first() {
                None | Some(Value::Undefined) => StringUnits::from(","),
                Some(value) => value.to_string_units_checked()?,
            };
            match receiver {
                Value::Array(items) => Value::String(join_units(&items.borrow(), &separator)?),
                _ => Value::String(String::new().into()),
            }
        }
        // Every one of these is a plain transformation of its arguments, which is what keeps
        // them in this function rather than on the interpreter.
        Native::MathAbs => Value::Number(argument_number(arguments)?.abs()),
        Native::MathFloor => Value::Number(argument_number(arguments)?.floor()),
        Native::MathCeil => Value::Number(argument_number(arguments)?.ceil()),
        // Not `f64::round`, which rounds a half away from zero: JavaScript rounds a half toward
        // positive infinity, so `Math.round(-1.5)` is -1 rather than -2.
        Native::MathRound => {
            let value = argument_number(arguments)?;
            Value::Number(if !value.is_finite() {
                value
            } else {
                // `value + 0.5` is itself rounded: 0.49999999999999994 + 0.5 is exactly 1, and
                // above 2^52 every double is an integer the addition can push to the next one.
                // Comparing the fraction against the floor is exact.
                let floor = value.floor();
                let rounded = if value - floor >= 0.5 { floor + 1.0 } else { floor };
                // A negative input that rounds to zero gives *negative* zero, which the floor
                // has already thrown away. Only `Object.is` and `1 / x` can see the difference,
                // and a console shows it, so it is worth putting back. Negative zero itself is
                // one of them: -0 < 0 is false, so the sign bit is the test.
                if rounded == 0.0 && value.is_sign_negative() {
                    -0.0
                } else {
                    rounded
                }
            })
        }
        Native::MathTrunc => Value::Number(argument_number(arguments)?.trunc()),
        // `Math.sign` preserves the sign of zero and passes NaN through, which `signum` does not.
        Native::MathSign => {
            let value = argument_number(arguments)?;
            Value::Number(if value.is_nan() || value == 0.0 {
                value
            } else {
                value.signum()
            })
        }
        Native::MathSqrt => Value::Number(argument_number(arguments)?.sqrt()),
        Native::MathCbrt => Value::Number(argument_number(arguments)?.cbrt()),
        Native::MathSinh => Value::Number(argument_number(arguments)?.sinh()),
        Native::MathCosh => Value::Number(argument_number(arguments)?.cosh()),
        Native::MathTanh => Value::Number(argument_number(arguments)?.tanh()),
        Native::MathAsinh => Value::Number(argument_number(arguments)?.asinh()),
        Native::MathAcosh => Value::Number(argument_number(arguments)?.acosh()),
        Native::MathAtanh => Value::Number(argument_number(arguments)?.atanh()),
        // `ln(1 + x)` and `e^x - 1` as one operation each, which is the whole point of both: the
        // two-step spelling loses every significant digit for small `x`.
        Native::MathLog1p => Value::Number(argument_number(arguments)?.ln_1p()),
        Native::MathExpm1 => Value::Number(argument_number(arguments)?.exp_m1()),
        // The nearest `f32`, widened back. A round trip, not a truncation.
        #[expect(clippy::cast_possible_truncation)]
        Native::MathFround => Value::Number(f64::from(argument_number(arguments)? as f32)),
        // Through the bit pattern rather than through `as f32`: rounding to half and rounding to
        // single are different roundings, and doing the second one twice would answer with a
        // value half cannot hold.
        Native::MathF16Round => {
            Value::Number(f64::from(half_to_f32(f64_to_half(argument_number(arguments)?))))
        }
        Native::MathClz32 => Value::Number(f64::from(to_uint32(argument_number(arguments)?).leading_zeros())),
        // Both operands are truncated to 32-bit *first*, which is what makes this different from
        // `a * b`: the product is exact in 32 bits and wraps rather than rounding.
        Native::MathImul => Value::Number(f64::from(
            to_int32(argument_number(arguments)?)
                .wrapping_mul(to_int32(argument_number_at(arguments, 1)?)),
        )),
        // `hypot` is not `sqrt(x*x + y*y)`: squaring overflows for arguments whose result is
        // perfectly representable, which is the whole reason it exists.
        Native::MathHypot => {
            let mut total = 0.0f64;
            for (index, _) in arguments.iter().enumerate() {
                total = total.hypot(argument_number_at(arguments, index)?);
            }
            Value::Number(total)
        }
        Native::MathAtan2 => Value::Number(
            argument_number(arguments)?.atan2(argument_number_at(arguments, 1)?),
        ),
        Native::MathSin => Value::Number(argument_number(arguments)?.sin()),
        Native::MathCos => Value::Number(argument_number(arguments)?.cos()),
        Native::MathTan => Value::Number(argument_number(arguments)?.tan()),
        Native::MathAsin => Value::Number(argument_number(arguments)?.asin()),
        Native::MathAcos => Value::Number(argument_number(arguments)?.acos()),
        Native::MathAtan => Value::Number(argument_number(arguments)?.atan()),
        // Number::exponentiate (ECMA-262 §6.1.6.1.3) differs from C's pow where IEEE 754 says
        // "1": a NaN exponent is NaN, and so is a base of +1 or -1 to an infinite power.
        Native::MathPow => {
            let base = argument_number(arguments)?;
            let exponent = argument_number_at(arguments, 1)?;
            Value::Number(
                if exponent.is_nan() || (base.abs() == 1.0 && exponent.is_infinite()) {
                    f64::NAN
                } else {
                    base.powf(exponent)
                },
            )
        }
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
            for (index, _) in arguments.iter().enumerate() {
                let value = argument_number_at(arguments, index)?;
                if value.is_nan() {
                    best = f64::NAN;
                } else if !best.is_nan() && ((smallest && value < best) || (!smallest && value > best)) {
                    best = value;
                }
            }
            Value::Number(best)
        }
        Native::MathLog => Value::Number(argument_number(arguments)?.ln()),
        Native::MathLog2 => Value::Number(argument_number(arguments)?.log2()),
        Native::MathLog10 => Value::Number(argument_number(arguments)?.log10()),
        Native::MathExp => Value::Number(argument_number(arguments)?.exp()),
        // `Number(x)` converts; `Number()` with no argument is 0, not NaN.
        Native::PrimitiveValueOf => receiver.clone(),
        Native::StringValueOf | Native::StringToStringMethod => match receiver {
            Value::String(_) => receiver.clone(),
            _ => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "String.prototype method called on incompatible receiver".to_owned(),
                });
            }
        },
        Native::SymbolValueOf => match receiver {
            Value::Symbol(_) => receiver.clone(),
            _ => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Symbol.prototype.valueOf called on incompatible receiver".to_owned(),
                });
            }
        },
        Native::BigIntValueOf => match receiver {
            Value::BigInt(_) => receiver.clone(),
            _ => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "BigInt.prototype.valueOf called on incompatible receiver".to_owned(),
                });
            }
        },
        // Every hint answers the same thing, which is the point: a symbol does not become a
        // number or a string, so the reduction ends here and the caller refuses it.
        Native::SymbolToPrimitive => match receiver {
            Value::Symbol(_) => receiver.clone(),
            // `thisSymbolValue` accepts the box as well as the primitive, which is what makes
            // `Object(sym)[Symbol.toPrimitive]()` answer the symbol it wraps.
            boxed if matches!(wrapped_primitive(boxed), Some(Value::Symbol(_))) => {
                wrapped_primitive(boxed).unwrap_or(Value::Undefined)
            }
            other => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!(
                        "Symbol.prototype[Symbol.toPrimitive] called on {}",
                        other.type_name()
                    ),
                });
            }
        },
        Native::BooleanToString => Value::String(branded_boolean(receiver)?.to_string().into()),
        Native::BooleanValueOf => Value::Boolean(branded_boolean(receiver)?),
        Native::NumberValueOf => Value::Number(branded_number(receiver)?),
        Native::NumberToString => {
            let value = branded_number(receiver)?;
            match arguments.first().map(Value::as_number) {
                // Radix 10 is the default and is exactly `String(n)`, exponent rules included.
                None | Some(10.0) => Value::String(number_string(value).into()),
                Some(radix) if (2.0..=36.0).contains(&radix) => {
                    Value::String(number_in_radix(value, radix as u32).into())
                }
                // A radix outside 2..=36 is a RangeError; returning NaN would look like a result.
                _ => return Err(ExecutionError::RadixOutOfRange),
            }
        }
        Native::NumberToExponential => {
            let value = branded_number(receiver)?;
            let digits = arguments.first().map_or(&Value::Undefined, |value| value);
            let requested = match digits {
                Value::Undefined => None,
                other => {
                    let digits = to_integer_or_infinity(other.as_number());
                    // The range check runs even for a value that is not finite, which is why an
                    // infinite receiver is answered *after* it rather than before.
                    if !(0.0..=100.0).contains(&digits) && value.is_finite() {
                        return Err(ExecutionError::RadixOutOfRange);
                    }
                    Some(digits.trunc().clamp(0.0, 100.0) as usize)
                }
            };
            Value::String(number_to_exponential(value, requested).into())
        }
        Native::NumberToPrecision => {
            let value = branded_number(receiver)?;
            match arguments.first() {
                // With no argument at all this is `toString`, not a one-digit rendering.
                None | Some(Value::Undefined) => {
                    Value::String(number_string(value).into())
                }
                Some(other) => {
                    let digits = to_integer_or_infinity(other.as_number());
                    if !value.is_finite() {
                        return Ok(Value::String(number_string(value).into()));
                    }
                    if !(1.0..=100.0).contains(&digits) {
                        return Err(ExecutionError::RadixOutOfRange);
                    }
                    Value::String(number_to_precision(value, digits.trunc() as usize).into())
                }
            }
        }
        Native::BigIntAsIntN | Native::BigIntAsUintN => {
            // `ToIndex`: the bit count is truncated towards zero and NaN is zero, so
            // `BigInt.asIntN(8.5, x)` is `asIntN(8, x)` rather than a RangeError. Only the range
            // itself refuses.
            let bits = to_integer_or_infinity(arguments.first().map_or(0.0, Value::as_number));
            if !(0.0..=9_007_199_254_740_991.0).contains(&bits) {
                return Err(ExecutionError::RadixOutOfRange);
            }
            let Some(Value::BigInt(value)) = arguments.get(1) else {
                return Err(ExecutionError::NotABigInt);
            };
            match truncate_bigint(value, bits as u64, matches!(native, Native::BigIntAsIntN)) {
                Some(truncated) => Value::BigInt(Rc::new(truncated)),
                // The answer would be a number this many bits wide, which is past what any
                // engine will build. Refused rather than approximated.
                None => return Err(ExecutionError::RadixOutOfRange),
            }
        }
        Native::NumberToFixed => {
            let value = branded_number(receiver)?;
            // `ToIntegerOrInfinity`, which answers zero for `undefined` and for NaN. Compared
            // raw, both were NaN and failed the range test below -- so `(1.5).toFixed()` gave
            // "2" and `(1.5).toFixed(undefined)` threw, for an argument the language says means
            // the same thing.
            let digits = to_integer_or_infinity(arguments.first().map_or(0.0, Value::as_number));
            if !(0.0..=100.0).contains(&digits) {
                return Err(ExecutionError::RadixOutOfRange);
            }
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Value::String(number_to_fixed(value, digits as u32).into())
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
        Native::IsNaN => Value::Boolean(argument_number(arguments)?.is_nan()),
        Native::IsFinite => Value::Boolean(argument_number(arguments)?.is_finite()),
        Native::ParseInt => parse_int(arguments),
        Native::ParseFloat => parse_float(arguments),
        // Serialising reads every property, and reading an accessor is a call.
        Native::JsonStringify => {
            unreachable!("`JSON.stringify` is dispatched by the interpreter")
        }
        // Reached only when there is no reviver: with one, `JSON.parse` has to call back into
        // the program, so the machine takes it. Both go through `parse_json_text`, so the two
        // entry points cannot disagree about what the grammar is.
        Native::JsonParse => {
            let text = match arguments.first() {
                None | Some(Value::Undefined) => StringUnits::from("undefined"),
                Some(value) => value.to_string_units_checked()?,
            };
            parse_json_text(&text)?
        }
        Native::ErrorCaptureStackTrace => Value::Undefined,
        // Array and typed-array iterators retain their receiver rather than snapshotting it.
        // State lives in reserved properties on the iterator itself, so `next` can observe a
        // write that happened after iterator creation, as the language requires.
        Native::ArrayEntries | Native::ArrayKeys | Native::ArrayValues => match receiver {
            Value::Array(_) | Value::TypedArray(_) => array_iterator_over(native, receiver),
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
                // `dispatch_native` coerces the receiver ahead of this, and the machine's own
                // direct calls always pass a string; anything else here is the `TypeError` the
                // coercion would have raised.
                return Err(ExecutionError::TypeErrorMessage {
                    message: "String.prototype method called on a non-string".to_owned(),
                });
            };
            // `"a".includes(/a/)` is refused rather than answered: the three searching methods
            // take a string needle, and a regexp argument is a program that meant `match` or
            // `search` and would otherwise get a silent answer about the regexp's spelling.
            let searching = match native {
                Native::StringIncludes => Some("includes"),
                Native::StringStartsWith => Some("startsWith"),
                Native::StringEndsWith => Some("endsWith"),
                _ => None,
            };
            if let Some(method) = searching
                && matches!(arguments.first(), Some(Value::RegExp(_)))
            {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!(
                        "First argument to String.prototype.{method} must not be a regular \
                         expression"
                    ),
                });
            }
            let argument = |index: usize| {
                arguments
                    .get(index)
                    .map_or_else(String::new, Value::to_display_string)
            };
            match native {
                Native::StringToUpperCase => Value::String(string_case_map(text, true)),
                Native::StringToLowerCase => Value::String(string_case_map(text, false)),
                Native::StringTrim => Value::String(string_trim_units(text, true, true)),
                // All three take a position, and all three ignored it: `"hello".startsWith("ell", 1)`
                // is true and read as false, which is a wrong answer rather than a missing
                // feature. Counted in UTF-16 units, like every other position the language
                // reports, so a position inside an astral character lands where the language
                // says it does rather than where Rust's byte offsets would.
                Native::StringIncludes | Native::StringStartsWith | Native::StringEndsWith => {
                    let haystack: Vec<u16> = text.units().collect();
                    let needle: Vec<u16> = match arguments.first() {
                        None => StringUnits::from("undefined").units().collect(),
                        Some(Value::String(value)) => value.encode_utf16().collect(),
                        Some(value) => value.to_string_units_checked()?.encode_utf16().collect(),
                    };
                    let clamp = |raw: f64| -> usize {
                        if raw.is_nan() || raw <= 0.0 {
                            0
                        } else if !raw.is_finite() {
                            haystack.len()
                        } else {
                            (raw.trunc() as usize).min(haystack.len())
                        }
                    };
                    // `endsWith`'s argument is where the *match ends*, and leaving it out means
                    // the end of the string -- not position zero, which is what the shared
                    // default would give it.
                    let position = match arguments.get(1) {
                        None | Some(Value::Undefined)
                            if matches!(native, Native::StringEndsWith) =>
                        {
                            haystack.len()
                        }
                        other => clamp(other.map_or(0.0, Value::as_number)),
                    };
                    Value::Boolean(match native {
                        Native::StringEndsWith => position
                            .checked_sub(needle.len())
                            .is_some_and(|start| haystack[start..position] == needle[..]),
                        Native::StringStartsWith => {
                            position + needle.len() <= haystack.len()
                                && haystack[position..position + needle.len()] == needle[..]
                        }
                        _ => {
                            needle.is_empty() && position <= haystack.len()
                                || haystack[position..]
                                    .windows(needle.len().max(1))
                                    .any(|window| window == needle.as_slice())
                        }
                    })
                }
                Native::StringIndexOf => {
                    // Both the position and result are UTF-16 code-unit offsets. Slicing Rust
                    // strings by bytes made the optional start unusable and would split astral
                    // characters; searching encoded units keeps all three quantities in the
                    // unit JavaScript specifies.
                    let haystack: Vec<u16> = text.units().collect();
                    let needle: Vec<u16> = argument(0).encode_utf16().collect();
                    let raw_start = arguments.get(1).map_or(0.0, Value::as_number);
                    let start = if raw_start.is_nan() || raw_start <= 0.0 {
                        0
                    } else if !raw_start.is_finite() {
                        haystack.len()
                    } else {
                        (raw_start.trunc() as usize).min(haystack.len())
                    };
                    let found = if needle.is_empty() {
                        Some(start)
                    } else {
                        haystack[start..]
                            .windows(needle.len())
                            .position(|window| window == needle)
                            .map(|offset| start + offset)
                    };
                    Value::Number(found.map_or(-1.0, |index| index as f64))
                }
                Native::StringRepeat => {
                    let count = arguments.first().map_or(0.0, Value::as_number);
                    if text.is_empty() || count < 0.0 || !count.is_finite() {
                        Value::String(String::new().into())
                    } else {
                        // Checked before the allocation rather than after: `repeat` is handed a
                        // count straight from the program, and `"x".repeat(2 ** 40)` is a
                        // request no engine grants.
                        bounded_string(
                            (text.len() as f64 * count).min(f64::from(u32::MAX) * 4.0) as usize,
                        )?;
                        let units: Vec<u16> = text.units().collect();
                        let mut repeated = Vec::with_capacity(units.len().saturating_mul(count as usize));
                        for _ in 0..count as usize {
                            repeated.extend_from_slice(&units);
                        }
                        Value::String(StringUnits::from_units(&repeated))
                    }
                }
                Native::StringSplit => {
                    // The limit is a count of *pieces*, not of separators, and it is read before
                    // the separator is: a limit of zero answers the empty array without the
                    // string ever being cut.
                    let limit = match arguments.get(1) {
                        None | Some(Value::Undefined) => u32::MAX as usize,
                        Some(value) => to_uint32(value.as_number()) as usize,
                    };
                    let separator = match arguments.first() {
                        Some(value) => value_to_string_units(value)?,
                        None => Vec::new(),
                    };
                    let parts: Vec<Value> = if limit == 0 {
                        Vec::new()
                    } else if matches!(arguments.first(), None | Some(Value::Undefined)) {
                        // An absent separator does not cut at all: the whole string is the one
                        // piece. `undefined` means absent here, which is why this asks the value
                        // rather than the argument count -- `"a".split(undefined)` is `["a"]`.
                        vec![Value::String(text.clone().into())]
                    } else if separator.is_empty() {
                        // An empty separator splits into characters, not into nothing.
                        text.units()
                            .take(limit)
                            .map(|unit| Value::String(StringUnits::from_units(&[unit])))
                            .collect()
                    } else {
                        let source: Vec<u16> = text.units().collect();
                        let mut parts = Vec::new();
                        let mut start = 0;
                        // The limit counts pieces, and a truncated split drops the
                        // tail rather than keeping it: `"a b c".split(" ", 1)` is
                        // `["a"]`, not `["a b c"]`.
                        while parts.len() < limit {
                            let Some(offset) = source[start..]
                                .windows(separator.len())
                                .position(|window| window == separator.as_slice())
                            else {
                                parts.push(Value::String(StringUnits::from_units(
                                    &source[start..],
                                )));
                                break;
                            };
                            let end = start + offset;
                            parts.push(Value::String(StringUnits::from_units(&source[start..end])));
                            start = end + separator.len();
                        }
                        parts
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
                    //
                    // Read in place: copying the units out made every call O(length), and a
                    // loop over the string quadratic (NOTES.md, 2026-09-18).
                    let length = text.unit_len();
                    let index = to_integer_or_infinity(
                        arguments.first().map_or(0.0, Value::as_number),
                    );
                    let index = if index < 0.0 {
                        length as f64 + index
                    } else {
                        index
                    };
                    match unit_in_range(text, index) {
                        Some(unit) => Value::String(StringUnits::from_units(&[unit])),
                        None => Value::Undefined,
                    }
                }
                Native::StringCharAt => {
                    // Code units, not characters: `charAt` counts what `length` counts, so half
                    // of a surrogate pair is a position and reads as that half.
                    let index = to_integer_or_infinity(
                        arguments.first().map_or(0.0, Value::as_number),
                    );
                    // Out of range is the empty string, not undefined.
                    match unit_in_range(text, index) {
                        Some(unit) => Value::String(StringUnits::from_units(&[unit])),
                        None => Value::String(String::new().into()),
                    }
                }
                Native::StringCharCodeAt => {
                    let index = to_integer_or_infinity(
                        arguments.first().map_or(0.0, Value::as_number),
                    );
                    // UTF-16 code units, so an astral character has two and this reads one.
                    match unit_in_range(text, index) {
                        Some(unit) => Value::Number(f64::from(unit)),
                        None => Value::Number(f64::NAN),
                    }
                }
                Native::StringPadStart | Native::StringPadEnd => {
                    let target = arguments.first().map_or(0.0, Value::as_number);
                    let current = text.unit_len();
                    let target = if target.is_finite() && target > 0.0 {
                        target as usize
                    } else if target.is_infinite() && target.is_sign_positive() {
                        usize::MAX
                    } else {
                        0
                    };
                    if target <= current {
                        return Ok(Value::String(text.clone().into()));
                    }
                    let padding: Vec<u16> = match arguments.get(1) {
                        None | Some(Value::Undefined) => vec![u16::from(b' ')],
                        Some(Value::String(value)) => value.units().collect(),
                        Some(value) => value
                            .to_string_units_checked()?
                            .units()
                            .collect(),
                    };
                    // Padding with the empty string cannot make progress, so it is left alone
                    // rather than looping forever.
                    if padding.is_empty() {
                        Value::String(text.clone().into())
                    } else {
                        bounded_string(target)?;
                        let mut filler = Vec::new();
                        while filler.len() < target - current {
                            filler.extend_from_slice(&padding);
                        }
                        filler.truncate(target - current);
                        let mut units = if matches!(native, Native::StringPadStart) {
                            filler.clone()
                        } else {
                            text.units().collect::<Vec<_>>()
                        };
                        if matches!(native, Native::StringPadStart) {
                            units.extend(text.units());
                        } else {
                            units.extend(filler);
                        }
                        Value::String(StringUnits::from_units(&units))
                    }
                }
                // String patterns only. A regular expression is a different search and a
                // different replacement grammar, and neither exists yet.
                Native::StringReplace | Native::StringReplaceAll => {
                    let pattern = value_to_string_units(
                        arguments.first().unwrap_or(&Value::Undefined),
                    )?;
                    let replacement = value_to_string_units(
                        arguments.get(1).unwrap_or(&Value::Undefined),
                    )?;
                    // The replacement is a *template*, not literal text, even when the pattern
                    // is a plain string: `$&` is what matched, `` $` `` and `$'` are what came
                    // before and after, and `$$` is one dollar. Treating it literally is the
                    // kind of wrong answer that only shows up in output nobody reads closely.
                    // The replacement is a *template*, not literal text, even when the pattern
                    // is a plain string: `$&` is what matched, `` $` `` and `$'` are what came
                    // before and after it, and `$$` is one dollar. The same expander the regexp
                    // path uses does it, so the two cannot disagree.
                    let all = matches!(native, Native::StringReplaceAll);
                    let subject: Vec<u16> = text.units().collect();
                    let needle = pattern;
                    let mut out = Vec::with_capacity(subject.len());
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
                        push_bounded_units(&mut out, &subject[at..start])?;
                        let expanded = expand_replacement(
                            &replacement,
                            &subject,
                            &(start..start + needle.len()),
                            &[],
                            &[],
                        )?;
                        push_bounded_units(&mut out, &expanded)?;
                        at = start + needle.len();
                        if !all {
                            break;
                        }
                        if needle.is_empty() {
                            if start < subject.len() {
                                // Carry across the unit the empty match stepped over.
                                push_bounded_units(&mut out, &subject[start..start + 1])?;
                                at = start + 1;
                            } else {
                                at = subject.len() + 1;
                                break;
                            }
                        }
                    }
                    if at <= subject.len() {
                        push_bounded_units(&mut out, &subject[at..subject.len()])?;
                    }
                    Value::String(StringUnits::from_units(&out))
                }
                Native::StringLastIndexOf => {
                    let needle = arguments
                        .first()
                        .map_or_else(String::new, Value::to_display_string);
                    // The index counts what `length` counts, which is UTF-16 code units --
                    // not bytes, and not characters either. The whole search is done in those
                    // units rather than in bytes because the *position* argument is measured in
                    // them, and that argument was being dropped: `"abcabc".lastIndexOf("c", 1)`
                    // answered 5, which is not a later match than the caller allowed but the
                    // last one in the string, found by a search that never looked at the bound.
                    let units: Vec<u16> = text.units().collect();
                    let needle_units: Vec<u16> = needle.encode_utf16().collect();
                    // The one position argument in the language where NaN means the *end* of
                    // the string rather than the start: the specification names it as
                    // `+Infinity`, which is also what a missing argument gives, so both search
                    // the whole string.
                    let requested = arguments.get(1).map_or(f64::NAN, Value::as_number);
                    let start = if requested.is_nan() {
                        units.len()
                    } else {
                        to_integer_or_infinity(requested).clamp(0.0, units.len() as f64) as usize
                    };
                    let found = units
                        .len()
                        .checked_sub(needle_units.len())
                        .and_then(|last| {
                            (0..=last.min(start))
                                .rev()
                                .find(|at| units[*at..*at + needle_units.len()] == needle_units[..])
                        });
                    Value::Number(found.map_or(-1.0, |at| at as f64))
                }
                // `substring` clamps both ends into range and swaps them if they are the wrong
                // way round, where `slice` counts a negative from the end and yields nothing
                // when they cross. The two look alike and agree on almost no edge case.
                Native::StringSubstring => {
                    let units: Vec<u16> = text.units().collect();
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
                    Value::String(StringUnits::from_units(&units[start..end]))
                }
                Native::StringSubstr => {
                    let units: Vec<u16> = text.units().collect();
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
                    Value::String(StringUnits::from_units(&units[start..start + count]))
                }
                Native::StringCodePointAt => {
                    let index = arguments.first().map_or(0.0, Value::as_number);
                    if index < 0.0 || index >= text.unit_len() as f64 {
                        Value::Undefined
                    } else {
                        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        let at = index as usize;
                        let lead = text.unit_at(at).unwrap_or(0);
                        // A surrogate *pair* reads as the whole code point, which is the entire
                        // difference from `charCodeAt`. A lone surrogate reads as itself: the
                        // answer is a number, so there is no string here to be unable to hold one
                        // (ADR 0022). Decoding through a `String` answered U+FFFD instead, which
                        // is what the trailing half of an emoji became.
                        let paired = if (0xD800..=0xDBFF).contains(&lead) {
                            text.unit_at(at + 1)
                                .filter(|low| (0xDC00..=0xDFFF).contains(low))
                                .map(|low| {
                                    0x10000
                                        + ((u32::from(lead) - 0xD800) << 10)
                                        + (u32::from(low) - 0xDC00)
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
                    Value::Number(match text.units().cmp(other.encode_utf16()) {
                        std::cmp::Ordering::Less => -1.0,
                        std::cmp::Ordering::Equal => 0.0,
                        std::cmp::Ordering::Greater => 1.0,
                    })
                }
                // The tables are ICU4X's, shared with the collator (`kiln_intl::normalize`).
                // This was the identity until 2026-09-19, which was honest for text a bundler
                // emits and wrong for `ﬁ` and every combining sequence a form tool produces.
                Native::StringNormalize => {
                    // A form outside the four the specification names is a `RangeError`:
                    // answering the string for `normalize("XYZ")` told the caller its text was
                    // normalised into a form that does not exist.
                    let form = match arguments.first() {
                        None | Some(Value::Undefined) => "NFC".to_owned(),
                        Some(value) => value.to_display_string(),
                    };
                    if !matches!(form.as_str(), "NFC" | "NFD" | "NFKC" | "NFKD") {
                        return Err(ExecutionError::RadixOutOfRange);
                    }
                    if text.is_empty() || text.is_ascii() {
                        Value::String(text.clone())
                    } else {
                        let units: Vec<u16> = text.units().collect();
                        let Some(normalized) = kiln_intl::normalize::normalize(&units, &form) else {
                            return Err(ExecutionError::RadixOutOfRange);
                        };
                        Value::String(StringUnits::from_units(&normalized))
                    }
                }
                Native::StringTrimStart => {
                    Value::String(string_trim_units(text, true, false))
                }
                Native::StringTrimEnd => {
                    Value::String(string_trim_units(text, false, true))
                }
                Native::StringConcat => {
                    let mut joined = text.to_units().into_vec();
                    for argument in arguments {
                        match argument {
                            Value::String(argument) => joined.extend(argument.units()),
                            _ => joined.extend(argument.to_string_units_checked()?.units()),
                        }
                    }
                    Value::String(StringUnits::from_units(&joined))
                }
                Native::StringSlice => {
                    // Code units, because `length` reports code units and a program that slices
                    // to `length` has to get the whole string back. Counting characters here
                    // made `s.slice(0, s.length)` shorter than `s` for any string holding an
                    // astral character, and made `slice` disagree with `[i]` about where a
                    // position is -- silently, in both cases.
                    let units: Vec<u16> = text.units().collect();
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
                    Value::String(StringUnits::from_units(&units[start.min(end)..end]))
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
            let characters = string_code_point_units(text)
                .into_iter()
                .map(Value::String)
                .collect::<Vec<_>>();
            let mut iterator = ObjectData::default();
            iterator.set_prototype(Value::Native(
                Native::PrototypeOf(PrototypeTable::IteratorFor(IteratorKind::String)),
                RealmId::PRIMARY,
            ));
            iterator.set("@@iterated", Value::Array(ArrayData::new(characters)));
            iterator.set("@@index", Value::Number(0.0));
            Value::Object(tracked_object(iterator))
        }
        // The state shape is the array iterator's, so the same stepper serves both.
        Native::StringIteratorNext => {
            call_native(intrinsics, Native::ArrayIteratorNext, receiver, arguments)?
        }
        // `Error.isError`: whether this value carries the internal slot every error is built
        // with. The stack trace remains a separate reserved property for diagnostics, but it is
        // not a brand because program code can add or remove any string property.
        Native::ErrorIsError => Value::Boolean(matches!(
            arguments.first(),
            Some(Value::Object(object)) if object.borrow().error_brand
        )),
        Native::WeakSetAdd | Native::WeakMapSet => {
            let is_map = native == Native::WeakMapSet;
            let key = arguments.first().cloned().unwrap_or(Value::Undefined);
            let value = arguments.get(1).cloned().unwrap_or(Value::Undefined);
            let Value::Object(object) = receiver else {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "incompatible weak collection receiver".to_owned(),
                });
            };
            let mut object = object.borrow_mut();
            let actual_map = matches!(object.get_own("@@map"), Some(&Value::Boolean(true)));
            let Some(entries) = object.weak_collection_entries.as_mut() else {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "incompatible weak collection receiver".to_owned(),
                });
            };
            if actual_map != is_map {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "incompatible weak collection receiver".to_owned(),
                });
            }
            let weak_key = WeakCollectionKey::new(&key).ok_or_else(|| {
                ExecutionError::TypeErrorMessage {
                    message: "value cannot be held weakly".to_owned(),
                }
            })?;
            entries.insert(
                weak_key,
                if is_map { value } else { Value::Undefined },
            );
            receiver.clone()
        }
        Native::WeakSetHas | Native::WeakMapHas | Native::WeakMapGet | Native::WeakSetDelete | Native::WeakMapDelete => {
            let is_map = matches!(native, Native::WeakMapHas | Native::WeakMapGet | Native::WeakMapDelete);
            let deleting = matches!(native, Native::WeakSetDelete | Native::WeakMapDelete);
            let getting = native == Native::WeakMapGet;
            let key = arguments.first().cloned().unwrap_or(Value::Undefined);
            let Value::Object(object) = receiver else {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "incompatible weak collection receiver".to_owned(),
                });
            };
            let mut object = object.borrow_mut();
            let actual_map = matches!(object.get_own("@@map"), Some(&Value::Boolean(true)));
            let Some(entries) = object.weak_collection_entries.as_mut() else {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "incompatible weak collection receiver".to_owned(),
                });
            };
            if actual_map != is_map {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "incompatible weak collection receiver".to_owned(),
                });
            }
            let identity = WeakCollectionKey::new(&key);
            if deleting {
                Value::Boolean(identity.is_some_and(|identity| entries.remove(&identity).is_some()))
            } else if getting {
                identity
                    .and_then(|identity| entries.get(&identity).cloned())
                    .unwrap_or(Value::Undefined)
            } else {
                Value::Boolean(identity.is_some_and(|identity| entries.contains_key(&identity)))
            }
        }
        Native::SetAdd | Native::MapSet => {
            let (entries, is_map) = require_collection(receiver, native)?;
            let key = arguments.first().cloned().unwrap_or(Value::Undefined);
            let value = arguments.get(1).cloned().unwrap_or(Value::Undefined);
            let existing = find_collection_entry(&entries, &key, is_map);
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
                        Value::Array(ArrayData::new(vec![key.clone(), value]))
                    } else {
                        key.clone()
                    };
                    let new_idx = entries.borrow().len();
                    entries.borrow_mut().push(entry);
                    collection_entry_inserted(&entries, &key, new_idx);
                    bump_size(receiver, 1.0);
                }
            }
            receiver.clone()
        }
        Native::SetHas | Native::MapHas => {
            let (entries, is_map) = require_collection(receiver, native)?;
            let key = arguments.first().cloned().unwrap_or(Value::Undefined);
            let found = find_collection_entry(&entries, &key, is_map).is_some();
            Value::Boolean(found)
        }
        Native::MapGet => {
            let (entries, is_map) = require_collection(receiver, native)?;
            let key = arguments.first().cloned().unwrap_or(Value::Undefined);
            match find_collection_entry(&entries, &key, is_map) {
                Some(index) => match entries.borrow().get(index) {
                    Some(Value::Array(pair)) => {
                        pair.borrow().get(1).cloned().unwrap_or(Value::Undefined)
                    }
                    _ => Value::Undefined,
                },
                None => Value::Undefined,
            }
        }
        Native::CollectionClear(_) => {
            let (entries, _) = require_collection(receiver, native)?;
            // Every entry becomes a tombstone rather than the vector being emptied, for the same
            // reason a delete does: a live iterator holds an index into this vector, and
            // shortening it underneath one would walk past the end.
            let count = entries.borrow().len();
            for index in 0..count {
                entries.borrow_mut()[index] = Value::Native(Native::Tombstone, RealmId::PRIMARY);
            }
            collection_cleared(&entries);
            set_size(receiver, 0.0);
            Value::Undefined
        }
        Native::SetDelete | Native::MapDelete => {
            let (entries, is_map) = require_collection(receiver, native)?;
            let key = arguments.first().cloned().unwrap_or(Value::Undefined);
            let index = find_collection_entry(&entries, &key, is_map);
            match index {
                Some(index) => {
                    // A hole, not a removal: compacting here would shift every later element
                    // past a live iterator's cursor and skip one.
                    entries.borrow_mut()[index] = Value::Native(Native::Tombstone, RealmId::PRIMARY);
                    collection_entry_deleted(&entries, &key);
                    bump_size(receiver, -1.0);
                    Value::Boolean(true)
                }
                None => Value::Boolean(false),
            }
        }
        Native::CollectionValues(_) | Native::CollectionKeys(_) | Native::CollectionEntries(_) => {
            let (entries, is_map) = require_collection(receiver, native)?;
            let mut iterator = ObjectData::default();
            iterator.set_prototype(Value::Native(
                Native::PrototypeOf(PrototypeTable::IteratorFor(if is_map {
                    IteratorKind::Map
                } else {
                    IteratorKind::Set
                })),
                RealmId::PRIMARY,
            ));
            iterator.set("@@iterated", Value::Array(entries));
            iterator.set("@@index", Value::Number(0.0));
            // What to hand back for each stored entry. Decided here, where the collection's kind
            // is known, so the iterator itself needs no reference back to it.
            iterator.set(
                "@@part",
                Value::String(
                    match (is_map, native) {
                        (true, Native::CollectionKeys(_)) => "key",
                        (true, Native::CollectionValues(_)) => "value",
                        (false, Native::CollectionEntries(_)) => "self-pair",
                        _ => "whole",
                    }
                    .to_owned().into(),
                ),
            );

            iterator.set(
                WellKnown::Iterator.property_key(),
                Value::Native(Native::IteratorSelf, RealmId::PRIMARY),
            );
            Value::Object(tracked_object(iterator))
        }
        Native::CollectionIteratorNext => {
            let iterated = iterator_state(receiver)?;
            let Value::Object(state) = receiver else {
                unreachable!("only an object carries the marker");
            };
            let mut index = state
                .borrow()
                .get("@@index")
                .unwrap_or(Value::Number(0.0))
                .as_number() as usize;
            let Value::Array(entries) = &iterated else {
                return Ok(iteration_result(Value::Undefined, true));
            };
            let part = state
                .borrow()
                .get("@@part")
                .map_or_else(String::new, |value| value.to_display_string());
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
                    Some(Value::Native(Native::Tombstone, _)) => index += 1,
                    Some(value) => {
                        state
                            .borrow_mut()
                            .set("@@index", Value::Number(index as f64 + 1.0));
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
            let iterated = iterator_state(receiver)?;
            let Value::Object(state) = receiver else {
                unreachable!("only an object carries the marker");
            };
            let (index, part) = {
                let state = state.borrow();
                (
                    state
                        .get("@@index")
                        .unwrap_or(Value::Number(0.0))
                        .as_number(),
                    state
                        .get("@@part")
                        .unwrap_or(Value::String("value".to_owned().into()))
                        .to_display_string(),
                )
            };
            let index = index as usize;
            // Re-read the length every step. An array shortened during iteration must stop
            // early rather than read past its end, which is exactly what a cached length would
            // do -- and is the mutation-during-iteration case the corpus checks.
            let item = match &iterated {
                Value::Array(items) => items.borrow().get(index).cloned(),
                Value::TypedArray(view) => view.get(index),
                _ => None,
            };
            match item {
                Some(value) => {
                    state
                        .borrow_mut()
                        .set("@@index", Value::Number(index as f64 + 1.0));
                    // An iterator walks *indices*, so it visits a hole -- and what it yields
                    // there is what a read yields, which is undefined.
                    let value = if matches!(value, Value::Native(Native::Hole, _)) {
                        Value::Undefined
                    } else {
                        value
                    };
                    let yielded = match part.as_str() {
                        "key" => Value::Number(index as f64),
                        "entry" => Value::Array(ArrayData::new(vec![
                            Value::Number(index as f64),
                            value,
                        ])),
                        _ => value,
                    };
                    iteration_result(yielded, false)
                }
                None => iteration_result(Value::Undefined, true),
            }
        }
        Native::SetConstructor | Native::MapConstructor => Value::Undefined,
        // `Array(3)` and `new Array(3)` are the same thing, which is unusual enough among the
        // constructors to be worth saying: the specification defines the call form as the
        // construct form, and libraries allocating a sized array reach for the shorter one.
        Native::ArrayConstructor => construct_array(arguments)?,
        // `Object(x)` passes an object through and boxes a primitive. The box is what makes
        // `Object(1) + 1` two rather than "[object Object]1": the wrapper carries the number,
        // and `ToPrimitive` gets it back through `valueOf`.
        Native::ObjectConstructor => match arguments.first() {
            Some(
                value @ (Value::Object(_)
                | Value::Array(_)
                | Value::Function(_)
                | Value::Native(_, _)
                | Value::Callback(_, _)
                | Value::Bound(_)
                | Value::Wrapped(_)
                | Value::Promise(_)
                | Value::Date(_)
                | Value::RegExp(_)
                | Value::TypedArray(_)
                | Value::ArrayBuffer(_)
                | Value::DataView(_)
                | Value::Proxy(_)
                | Value::Generator(_)),
            ) => value.clone(),
            Some(
                primitive @ (Value::String(_)
                | Value::Number(_)
                | Value::Boolean(_)
                | Value::Symbol(_)
                | Value::BigInt(_)),
            ) => wrap_primitive(primitive.clone()),
            // `Object()`, `Object(null)` and `Object(undefined)` all ask for a fresh object.
            _ => Value::Object(tracked_object(ObjectData::default())),
        },
        Native::NodeEventEmitter => node_event_emitter(),
        Native::NodeStringDecoderConstructor => string_decoder::node_string_decoder(),
        Native::NodeReadable => node_readable(arguments, Some(receiver)),
        Native::NodeWritable => node_writable(arguments, Some(receiver)),
        Native::NodeHttpServerResponse => node_writable(arguments, Some(receiver)),
        Native::NodeDiagnosticsChannel
        | Native::NodeDiagnosticsChannelCreate
        | Native::NodeDiagnosticsTracingChannel
        | Native::NodeDiagnosticsPublish
        | Native::NodeDiagnosticsRunStores
        | Native::NodeUtil
        | Native::NodeUtilInherits
        | Native::NodeUtilDebuglog
        | Native::NodeUtilDebugNoop
        | Native::NodeUtilDeprecate
        | Native::NodeUtilInspect
        | Native::NodeUtilFormat
        | Native::ConsoleConstructor
        | Native::ConsoleLog
        | Native::ConsoleInfo
        | Native::ConsoleDebug
        | Native::ConsoleDir
        | Native::ConsoleWarn
        | Native::ConsoleError
        | Native::ConsoleAssert
        | Native::ConsoleClear
        | Native::ConsoleCount
        | Native::ConsoleCountReset
        | Native::ConsoleGroup
        | Native::ConsoleGroupEnd
        | Native::ConsoleGroupCollapsed
        | Native::ConsoleTable
        | Native::ConsoleTime
        | Native::ConsoleTimeEnd
        | Native::ConsoleTimeLog
        | Native::ConsoleTrace
        | Native::ConsoleDirxml
        | Native::ConsoleTimeStamp
        | Native::ConsoleProfile
        | Native::ConsoleProfileEnd
        | Native::NodeConsole
        | Native::NodeAssert
        | Native::NodeAssertOk
        | Native::NodeAsyncHooks
        | Native::NodeAsyncResource
        | Native::NodeAsyncResourceBind
        | Native::NodeAsyncResourceEmitDestroy
        | Native::NodeCrypto
        | Native::NodeCryptoRandomUuid
        | Native::NodeCryptoRandomBytes
        | Native::NodeCryptoRandomFillSync
        | Native::NodeCryptoGetHashes
        | Native::NodeCryptoGetRandomValues
        | Native::NodeCryptoCreateHash
        | Native::NodeCryptoCreateHmac
        | Native::NodeCryptoTimingSafeEqual
        | Native::NodeCryptoKeyImport
        | Native::NodeCryptoKeyPublic
        | Native::NodeCryptoKeyInfo
        | Native::NodeCryptoKeyExport
        | Native::NodeCryptoKeyFree
        | Native::WebCryptoPrimitive
        | Native::WebCompressionPrimitive
        | Native::NodeHashUpdate
        | Native::NodeHashDigest
        | Native::NodeStringDecoder
        | Native::NodeStringDecoderWrite
        | Native::NodeStringDecoderEnd
        | Native::NodeStream
        | Native::NodeStreamFinished
        | Native::NodeDns
        | Native::NodeDnsLookup
        | Native::NodeOs
        | Native::NodeOsNetworkInterfaces
        | Native::NodeOsHostname
        | Native::NodeTty
        | Native::NodeTtyIsatty
        | Native::NodeZlib
        | Native::NodeZlibCreateDecompress
        | Native::NodeZlibCreateGunzip
        | Native::NodeZlibCreateInflate
        | Native::NodeZlibCreateGzip
        | Native::NodeZlibCreateDeflate
        | Native::NodeZlibCreateUnzip
        | Native::NodeZlibCreateBrotliDecompress
        | Native::NodeZlibGunzipSync
        | Native::NodeZlibInflateSync
        | Native::NodeZlibGzipSync
        | Native::NodeZlibDeflateSync
        | Native::NodeZlibUnzipSync
        | Native::NodePerfHooks
        | Native::NodeHttps
        | Native::NodeHttpsCreateServer
        | Native::NodeHttp2
        | Native::NodeHttp2CreateServer
        | Native::NodeHttp2CreateSecureServer
        | Native::NodeModule
        | Native::NodeModuleCreateRequire
        | Native::NodeModuleLoader
        | Native::NodePath
        | Native::NodePathJoin
        | Native::NodePathResolve
        | Native::NodePathIsAbsolute
        | Native::NodePathPart(_)
        | Native::NodePathRelative
        | Native::NodePathParse
        | Native::NodePathFormat
        | Native::NodeOsFact(_)
        | Native::NodeWorkerThreads
        | Native::NodeWorkerConstructor
        | Native::NodeQueryString
        | Native::NodeQueryStringParse
        | Native::NodeUrl
        | Native::NodeUrlParse
        | Native::NodeUrlFileUrlToPath
        | Native::NodeUrlPathToFileUrl
        | Native::NodeNet
        | Native::NodeNetCreateServer
        | Native::NodeNetConnect
        | Native::NodeSocketWrite
        | Native::NodeSocketEnd
        | Native::NodeSocketDestroy
        | Native::NodeSocketCork
        | Native::NodeSocketUncork
        | Native::NodeReadablePush
        | Native::NodeReadableRead
        | Native::NodeReadableDestroy
        | Native::NodeStreamAddAbortSignal
        | Native::NodeWritableWrite
        | Native::NodeWritableEnd
        | Native::NodeWritableDestroy
        | Native::NodeReadablePause
        | Native::NodeReadableResume
        | Native::NodeReadableSetEncoding
        | Native::NodeReadablePipe
        | Native::NodeReadableUnpipe
        | Native::NodeHttp
        | Native::NodeHttpServer
        | Native::NodeHttpCreateServer
        | Native::NodeHttpRequest
        | Native::NodeHttpGet
        | Native::NodeHttpCreateFetchServer
        | Native::NodeHttpFetchRespond
        | Native::NodeHttpCancelRequest
        | Native::NodeClientRequestWrite
        | Native::NodeClientRequestEnd
        | Native::NodeClientRequestAssignSocket
        | Native::NodeBuffer
        | Native::NodeBufferGlobal
        | Native::NodeBufferFrom
        | Native::NodeBufferAlloc
        | Native::NodeBufferIsBuffer
        | Native::NodeBufferByteLength
        | Native::NodeBufferConcat
        | Native::NodeBufferToString
        | Native::ProcessHrtime
        | Native::ProcessHrtimeBigInt
        | Native::ProcessCwd
        | Native::ProcessKill
        | Native::ProcessExit
        | Native::ProcessExitCodeGet
        | Native::ProcessExitCodeSet
        | Native::GcRequest
        | Native::ProcessStartRaw
        | Native::ProcessCancelRaw
        | Native::NodeChildProcessExecFileSync
        | Native::UrlParseRaw
        | Native::UrlSetRaw
        | Native::UrlFastRaw
        | Native::UrlAdoptRaw
        | Native::JobsPendingRaw
        | Native::QueueIoRaw
        | Native::TimerRefRaw
        | Native::WorkerOpRaw
        | Native::PortWaitRaw
        | Native::PromiseRejectionsRaw
        | Native::ProcessStdinSetRawMode
        | Native::UrlConstructor
        | Native::UrlSearchParamsGet
        | Native::UrlSearchParamsHas
        | Native::UrlSearchParamsGetAll
        | Native::UrlSearchParamsToString
        | Native::UrlToString
        | Native::TextDecoderConstructor
        | Native::TextDecoderDecode
        | Native::NodeServerListen
        | Native::NodeServerClose
        | Native::NodeServerAddress
        | Native::NodeServerIsListening
        | Native::NodeServerSetTimeout
        | Native::NodeResponseSetHeader
        | Native::NodeResponseGetHeader
        | Native::NodeResponseGetHeaders
        | Native::NodeResponseHasHeader
        | Native::NodeResponseRemoveHeader
        | Native::NodeResponseWriteHead
        | Native::NodeResponseWrite
        | Native::NodeResponseEnd => Value::Undefined,
        Native::Error(_)
        | Native::NodeEvents
        | Native::NodeFsPromises
        | Native::NodeFs
        | Native::NodeFsExistsSync
        | Native::NodeFsReadFileSync
        | Native::NodeFsMkdirSync
        | Native::NodeFsWriteFileSync
        | Native::NodeFsStatSync
        | Native::NodeFsLstatSync
        | Native::NodeFsFstatSync
        | Native::NodeFsReaddirSync
        | Native::NodeFsRmSync
        | Native::NodeFsUnlinkSync
        | Native::NodeFsRealpathSync
        | Native::NodeFsRenameSync
        | Native::NodeFsCopyFileSync
        | Native::NodeFsAccessSync
        | Native::NodeFsAppendFileSync
        | Native::NodeFsChmodSync
        | Native::NodeFsFchmodSync
        | Native::NodeFsOpenSync
        | Native::NodeFsCloseSync
        | Native::NodeFsReadlinkSync
        | Native::NodeFsRmdirSync
        | Native::NodeFsUtimesSync
        | Native::NodeFsFutimesSync
        | Native::NodeFsSync
        | Native::NodeFsWriteSync
        | Native::NodeFsFtruncateSync
        | Native::NodeFsFsyncSync
        | Native::AbortControllerConstructor
        | Native::AbortSignalConstructor
        | Native::AbortControllerAbort
        | Native::AbortSignalAbort
        | Native::AbortSignalTimeout
        | Native::AbortSignalAny
        | Native::AbortSignalListen { .. }
        | Native::AbortSignalThrowIfAborted
        | Native::AbortSignalSettle
        | Native::AbortSignalFollow
        | Native::TextEncoderConstructor
        | Native::TextEncoderEncode
        | Native::NodeFsReadFile
        | Native::NodeFsWriteFile
        | Native::NodeFsAppendFile
        | Native::NodeFsStat
        | Native::NodeFsLstat
        | Native::NodeFsMkdir
        | Native::NodeFsReaddir
        | Native::NodeFsCopyFile
        | Native::NodeFsRename
        | Native::NodeFsRm
        | Native::NodeFsUnlink
        | Native::NodeFsRealpath
        | Native::NodeFsAccess
        | Native::NodeFsChmod
        | Native::NodeFsRmdir
        | Native::NodeFsReadlink
        | Native::NodeStatIsFile
        | Native::NodeStatIsDirectory
        | Native::NodeStatIsSymbolicLink
        | Native::NodeEventOn
        | Native::NodeEventOnce
        | Native::NodeEventEmit
        | Native::NodeEventListeners
        | Native::NodeEventListenerCount
        | Native::NodeEventNames
        | Native::NodeEventRemoveListener
        | Native::NodeEventRemoveAllListeners
        | Native::NodeEventSetMaxListeners
        | Native::ProcessGetBuiltinModule
        | Native::SymbolConstructor
        // A namespace, not a function: `Math` has no call behaviour of its own, and reaching
        // here means something called it.
        | Native::Math
        | Native::Json
        | Native::AsyncFunctionPrototype
        | Native::GeneratorFunctionPrototype
        | Native::AsyncGeneratorFunctionPrototype
        | Native::WebStub(_) => Value::Undefined,
    })
}
