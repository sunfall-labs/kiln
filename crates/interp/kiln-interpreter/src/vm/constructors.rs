use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    pub(super) fn construct_exotic(
        &mut self,
        parent: Native,
        arguments: &[Value],
        current: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        Ok(Some(match parent {
            Native::Error(kind) => self.construct_error_reached(kind, arguments, current)?,
            Native::AbortControllerConstructor => abort_controller(),
            Native::TextEncoderConstructor => text_encoder(),
            // `new AbortSignal()` is a TypeError in every engine: a signal is produced by a
            // controller or by one of the statics, never constructed, and that is the whole of
            // how cancellation stays one-directional.
            Native::AbortSignalConstructor => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Illegal constructor".to_owned(),
                });
            }
            Native::NodeEventEmitter => node_event_emitter(),
            // `new Buffer(value)` retains Node's deprecated constructor behavior and shares the
            // same allocation semantics as the callable form and `Buffer.from`.
            Native::NodeBufferGlobal => {
                self.dispatch_native(parent, &Value::Undefined, arguments, current)?
            }
            Native::ConsoleConstructor => self.construct_console(arguments, current)?,
            Native::NodeStringDecoderConstructor => string_decoder::node_string_decoder(),
            Native::NodeReadable => node_readable(arguments, None),
            Native::NodeWritable => node_writable(arguments, None),
            Native::NodeHttpServerResponse => node_writable(arguments, None),
            Native::ArrayConstructor => construct_array(arguments)?,
            Native::SetConstructor
            | Native::MapConstructor
            | Native::WeakSetConstructor
            | Native::WeakMapConstructor => {
                self.construct_collection(parent, arguments, current)?
            }
            Native::WeakRefConstructor => self.construct_weak_ref(arguments, current)?,
            Native::ShadowRealmConstructor => self.construct_shadow_realm()?,
            Native::DisposableStackConstructor => new_disposable_stack(Disposal::Sync),
            Native::AsyncDisposableStackConstructor => new_disposable_stack(Disposal::Async),
            Native::FinalizationRegistryConstructor => {
                self.construct_finalization_registry(arguments, current)?
            }
            Native::DateConstructor | Native::RegExpConstructor => {
                self.dispatch_native(parent, &Value::Undefined, arguments, current)?
            }
            Native::TextDecoderConstructor => {
                self.dispatch_native(parent, &Value::Undefined, arguments, current)?
            }
            // `class T extends Boolean {}` gets a wrapper from `super()`, exactly as
            // `new Boolean(x)` does -- the derived constructor then hangs its own fields on it,
            // and the boxed primitive is still there for `valueOf` to find.
            Native::StringConstructor | Native::NumberConstructor | Native::BooleanConstructor => {
                let primitive =
                    self.dispatch_native(parent, &Value::Undefined, arguments, current)?;
                wrap_primitive(primitive)
            }
            // `class T extends Object {}`: `super(x)` is `Object(x)`, which boxes a primitive
            // and hands an object straight back.
            Native::ObjectConstructor => call_native(
                &self.intrinsics,
                Native::ObjectConstructor,
                &Value::Undefined,
                arguments,
            )?,
            // `Iterator` is abstract but deliberately *subclassable*: writing an iterator in
            // JavaScript is what it exists for, and such a class needs `super()` to work. Only a
            // direct `new Iterator()` is refused, and that never reaches here.
            Native::IteratorConstructor => {
                Value::Object(Rc::new(RefCell::new(ObjectData::default())))
            }
            // The rest of the constructible built-ins, each doing through `super()` exactly what
            // it does through `new`. They were reachable one way and not the other, which is not
            // a distinction the language draws: `class MyF64 extends Float64Array {}` is how
            // half of Test262's resizable-buffer corpus builds the view it then tests.
            // Same reduction as a direct `new`: a subclass's `super(buffer, offset)` converts
            // its arguments the way the base constructor does.
            Native::TypedArrayConstructor(_) => {
                let arguments = self.typed_array_arguments(arguments, current)?;
                call_native(&self.intrinsics, parent, &Value::Undefined, &arguments)?
            }
            Native::ArrayBufferConstructor => {
                self.construct_array_buffer(false, arguments, current)?
            }
            Native::SharedArrayBufferConstructor => {
                self.construct_array_buffer(true, arguments, current)?
            }
            Native::DataViewConstructor => self.construct_data_view(arguments, current)?,
            Native::WebStub(kind) => {
                let realm = self.current_realm;
                let mut obj = ObjectData::default();
                obj.set_prototype(Value::Native(
                    Native::PrototypeOf(PrototypeTable::WebStub(kind)),
                    realm,
                ));
                obj.define_value(
                    "constructor",
                    Value::Native(Native::WebStub(kind), realm),
                    Attributes::DEFAULT,
                );
                Value::Object(Rc::new(RefCell::new(obj)))
            }
            Native::PromiseConstructor => self.construct_promise(arguments, current)?,
            Native::FunctionConstructor => {
                self.function_from_source(arguments, current, "function")?
            }
            Native::AsyncFunctionConstructor => {
                self.function_from_source(arguments, current, "async function")?
            }
            Native::GeneratorFunctionConstructor => {
                self.function_from_source(arguments, current, "function*")?
            }
            Native::AsyncGeneratorFunctionConstructor => {
                self.function_from_source(arguments, current, "async function*")?
            }
            Native::UrlConstructor => self.dispatch_native(
                Native::UrlConstructor,
                &Value::Undefined,
                arguments,
                current,
            )?,
            Native::Intl(intl::Op::Constructor(service)) => {
                self.intl_construct(service, arguments, current)?
            }
            Native::Temporal(temporal::Op::Constructor(kind)) => {
                self.temporal_construct(kind, arguments, current)?
            }
            _ => return Ok(None),
        }))
    }

    /// `new callee(...arguments)`.
    ///
    /// Leaves the instance on the stack for a native constructor. For a user function it pushes a
    /// frame instead, and the instance arrives when that frame returns -- which is why this
    /// cannot simply hand one back.
    pub(super) fn construct(
        &mut self,
        callee: Value,
        arguments: Vec<Value>,
        current: usize,
    ) -> Result<(), ExecutionError> {
        // `new C()` is the case where the two coincide, which is every use of this but one.
        let new_target = callee.clone();
        self.construct_as(callee, arguments, new_target, current)
    }

    /// The same, with `new.target` named separately.
    ///
    /// They differ in exactly one operation -- `Reflect.construct(C, [], D)` -- and the
    /// difference is observable twice over: the body reads `new.target`, and the instance it was
    /// handed inherits from `D.prototype` rather than `C.prototype`.
    pub(super) fn construct_as(
        &mut self,
        callee: Value,
        arguments: Vec<Value>,
        new_target: Value,
        current: usize,
    ) -> Result<(), ExecutionError> {
        let realm = value_realm(&callee);
        self.entering(realm, |vm| {
            vm.construct_as_dispatch(callee, arguments, new_target, current)
        })
    }

    pub(super) fn construct_as_dispatch(
        &mut self,
        callee: Value,
        arguments: Vec<Value>,
        new_target: Value,
        current: usize,
    ) -> Result<(), ExecutionError> {
        // `OrdinaryCreateFromConstructor` for the built-ins: every one of them takes its
        // instance's prototype from `NewTarget` rather than from itself, which is only visible
        // when the two differ -- `Reflect.construct(Date, [0], D)` and the `super()` of a
        // subclass. Each branch below builds the value its own way and none of them was asking,
        // so `class D extends Date {}` produced a date whose prototype was `Date.prototype` and
        // `new D(0) instanceof D` was false. Decided here once rather than in eight
        // constructors, and only when the two differ: reading `prototype` off the target runs a
        // program's getter, and doing it on every `new Date()` would be a call nothing asked for.
        let subclassed = match (&callee, &new_target) {
            // `new Proxy` is the one built-in whose `[[Construct]]` ignores `NewTarget`: a proxy
            // has no prototype of its own, it forwards the question to its target.
            (Value::Native(Native::ProxyConstructor, _), _) => false,
            (Value::Native(built_in, _), Value::Native(target, _)) => built_in != target,
            (Value::Native(_, _), _) => true,
            _ => false,
        };
        let depth = self.stack.len();
        match callee {
            // `new Temporal.PlainDate(...)`. The engine builds the value; what is decided here is
            // only what the arguments meant, which is the half the language owns.
            Value::Native(Native::Temporal(temporal::Op::Constructor(kind)), _) => {
                let built = self.temporal_construct(kind, &arguments, current)?;
                self.stack.push(built);
            }
            // `new Proxy(target, handler)` is the constructor; a proxy *of* something
            // constructible is the `construct` trap.
            Value::Native(Native::ProxyConstructor, _) => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let handler = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                // Both have to be objects: every operation on a proxy asks one of them a
                // question, and a primitive has no answer to give. Refused where the proxy is
                // made rather than at the first read that finds nothing.
                if !is_an_object(&target) || !is_an_object(&handler) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Cannot create proxy with a non-object as target or handler"
                            .to_owned(),
                    });
                }
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
                            vec![inner, list, new_target.clone()],
                            current,
                        )?;
                        // `new` produces an object, so a `construct` trap that answered with
                        // anything else has produced something `new` cannot mean.
                        if !is_an_object(&built) {
                            return Err(Self::proxy_invariant("construct must report an object"));
                        }
                        self.stack.push(built);
                    }
                    None => return self.construct_as(inner, arguments, new_target, current),
                }
            }
            Value::Native(Native::Error(kind), _) => {
                let error = self.construct_error_reached(kind, &arguments, current)?;
                self.stack.push(error);
            }
            Value::Native(Native::ArrayConstructor, _) => {
                let array = construct_array(&arguments)?;
                self.stack.push(array);
            }
            Value::Native(Native::NodeEventEmitter, _) => {
                self.stack.push(node_event_emitter());
            }
            Value::Native(Native::ConsoleConstructor, _) => {
                let built = self.construct_console(&arguments, current)?;
                self.stack.push(built);
            }
            Value::Native(Native::NodeReadable, _) => {
                self.stack.push(node_readable(&arguments, None));
            }
            Value::Native(Native::NodeWritable, _) => {
                self.stack.push(node_writable(&arguments, None));
            }
            Value::Native(Native::NodeHttpServerResponse, _) => {
                self.stack.push(node_writable(&arguments, None));
            }
            Value::Native(Native::NodeHttpServer, _) => {
                let built = self.dispatch_node_http(
                    Native::NodeHttpServer,
                    &Value::Undefined,
                    &arguments,
                    current,
                )?;
                self.stack.push(built);
            }
            Value::Native(Native::NodeAsyncLocalStorage, _) => {
                self.stack.push(node_async_local_storage());
            }
            Value::Native(Native::NodeAsyncResource, _) => {
                self.stack.push(ordinary_object([
                    (
                        "bind".to_owned(),
                        Value::Native(Native::NodeAsyncResourceBind, RealmId::PRIMARY),
                    ),
                    (
                        "emitDestroy".to_owned(),
                        Value::Native(Native::NodeAsyncResourceEmitDestroy, RealmId::PRIMARY),
                    ),
                ]));
            }
            Value::Native(Native::UrlConstructor, _) => {
                let built = self.dispatch_native(
                    Native::UrlConstructor,
                    &Value::Undefined,
                    &arguments,
                    current,
                )?;
                self.stack.push(built);
            }
            Value::Native(Native::TextDecoderConstructor, _) => {
                let built = self.dispatch_native(
                    Native::TextDecoderConstructor,
                    &Value::Undefined,
                    &arguments,
                    current,
                )?;
                self.stack.push(built);
            }
            Value::Native(Native::NodeStringDecoderConstructor, _) => {
                self.stack.push(string_decoder::node_string_decoder());
            }
            // The call form delegates to ToObject. Construction shares that behavior unless
            // Reflect.construct supplied a distinct new.target, which the constructor branch
            // below preserves as an ordinary allocation.
            Value::Native(Native::ObjectConstructor, _) => {
                // `Object(value)` normally returns `value` when it is already an object. With
                // a distinct `new.target`, [[Construct]] instead performs
                // OrdinaryCreateFromConstructor and ignores that argument. The shared tail
                // below applies `new_target.prototype` (including its realm fallback).
                let object = if subclassed {
                    Value::Object(Rc::new(RefCell::new(ObjectData::default())))
                } else {
                    call_native(
                        &self.intrinsics,
                        Native::ObjectConstructor,
                        &Value::Undefined,
                        &arguments,
                    )?
                };
                self.stack.push(object);
            }
            // `new Uint8Array(arrayLike)` and `new Uint8Array(iterable)` read their elements
            // through the program: a `length` may be a getter and an iterator is a call, and the
            // pure constructor can do neither. It applied `ToIndex` to the object instead --
            // `NaN`, so zero -- and answered an *empty* view for every array-like there is,
            // which the corpus builds a typed array from on almost every path. `Array.from`
            // already decides between the two protocols, so the argument is collected with it,
            // exactly as `%TypedArray%.from` does.
            Value::Native(Native::TypedArrayConstructor(kind), _)
                if arguments.first().is_some_and(|argument| {
                    // A buffer, an array and another view each have a construction of their own
                    // above; every other object is one of the two protocols `Array.from` knows.
                    is_reflect_target(argument)
                        && !matches!(
                            argument,
                            Value::ArrayBuffer(_) | Value::Array(_) | Value::TypedArray(_)
                        )
                }) =>
            {
                let source = arguments.first().cloned().unwrap_or(Value::Undefined);
                let iterator =
                    self.read_property(&source, WellKnown::Iterator.property_key(), current)?;
                let built = if is_callable(&iterator) {
                    // An iterable has no count ahead of its elements: they arrive through calls,
                    // and `Array.from` is where that protocol is already written.
                    let collected = self.array_from(&Value::Undefined, &arguments[..1], current)?;
                    construct_typed_array(kind, &[collected])?
                } else {
                    // An array-like states its count first, so a `length` no buffer could hold
                    // is refused before anything is allocated -- rather than after materialising
                    // sixteen million `Value`s to copy eight-bit numbers out of.
                    let length = self.array_like_length(&source, current)?;
                    #[expect(clippy::cast_precision_loss)]
                    let requested = length as f64;
                    let count = bounded_buffer(requested, kind.width())?;
                    #[expect(clippy::cast_precision_loss)]
                    let view = construct_typed_array(kind, &[Value::Number(count as f64)])?;
                    let Value::TypedArray(elements) = &view else {
                        unreachable!("a typed array constructor answers with a view");
                    };
                    let elements = Rc::clone(elements);
                    for index in 0..count {
                        let value = self.element_get(&source, index as u64, current)?;
                        let element = self.as_element(kind, &value, current)?;
                        elements.set(index, &element)?;
                    }
                    view
                };
                self.stack.push(built);
            }
            // Through the machine, not `call_native`: the numeric arguments are reduced by
            // calling the program's `valueOf`, which the machine-free table cannot do. It
            // answered `NaN` for an object and `to_index` read that as zero, so
            // `new Uint8Array(buffer, {valueOf: () => 4})` silently viewed from offset *nought*.
            Value::Native(native @ Native::TypedArrayConstructor(_), _) => {
                let arguments = self.typed_array_arguments(&arguments, current)?;
                let built = call_native(&self.intrinsics, native, &Value::Undefined, &arguments)?;
                self.stack.push(built);
            }
            // Construction uses ordinary ToString after ToPrimitive, so a direct or boxed
            // Symbol throws here. The call form `String(symbol)` has a deliberate special case
            // that returns its description instead.
            Value::Native(Native::StringConstructor, _) => {
                let primitive = match arguments.first() {
                    Some(value) => self.as_primitive(value, PrimitiveHint::String, current)?,
                    None => Value::String(StringUnits::default()),
                };
                if matches!(primitive, Value::Symbol(_)) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "a symbol cannot be converted to a string".to_owned(),
                    });
                }
                self.stack.push(wrap_primitive(Value::String(
                    primitive.to_string_units_checked()?,
                )));
            }
            Value::Native(native @ (Native::NumberConstructor | Native::BooleanConstructor), _) => {
                let primitive =
                    self.dispatch_native(native, &Value::Undefined, &arguments, current)?;
                self.stack.push(wrap_primitive(primitive));
            }
            // `new Function(source)` and `Function(source)` build the same function: the
            // specification routes both through the same steps and differs only in a `NewTarget`
            // that decides which prototype the result inherits, which this realm does not model.
            Value::Native(Native::FunctionConstructor, _) => {
                let built = self.function_from_source(&arguments, current, "function")?;
                self.stack.push(built);
            }
            Value::Native(Native::AsyncFunctionConstructor, _) => {
                let built = self.function_from_source(&arguments, current, "async function")?;
                self.stack.push(built);
            }
            Value::Native(Native::GeneratorFunctionConstructor, _) => {
                let built = self.function_from_source(&arguments, current, "function*")?;
                self.stack.push(built);
            }
            Value::Native(Native::AsyncGeneratorFunctionConstructor, _) => {
                let built = self.function_from_source(&arguments, current, "async function*")?;
                self.stack.push(built);
            }
            Value::Native(Native::ArrayBufferConstructor, _) => {
                let built = self.construct_array_buffer(false, &arguments, current)?;
                self.stack.push(built);
            }
            Value::Native(Native::SharedArrayBufferConstructor, _) => {
                let built = self.construct_array_buffer(true, &arguments, current)?;
                self.stack.push(built);
            }
            Value::Native(Native::DataViewConstructor, _) => {
                let built = self.construct_data_view(&arguments, current)?;
                self.stack.push(built);
            }
            Value::Native(Native::WebStub(kind), _) => {
                let realm = self.current_realm;
                let mut obj = ObjectData::default();
                obj.set_prototype(Value::Native(
                    Native::PrototypeOf(PrototypeTable::WebStub(kind)),
                    realm,
                ));
                obj.define_value(
                    "constructor",
                    Value::Native(Native::WebStub(kind), realm),
                    Attributes::DEFAULT,
                );
                self.stack.push(Value::Object(Rc::new(RefCell::new(obj))));
            }
            // `new Intl.NumberFormat(...)`. The engine builds the formatter; what is decided
            // here is only what the options meant, which is the half the language owns.
            Value::Native(Native::Intl(intl::Op::Constructor(service)), _) => {
                let built = self.intl_construct(service, &arguments, current)?;
                self.stack.push(built);
            }
            Value::Native(Native::DateConstructor, _) => {
                let date = self.dispatch_native(
                    Native::DateConstructor,
                    &Value::Undefined,
                    &arguments,
                    current,
                )?;
                self.stack.push(date);
            }
            Value::Native(Native::RegExpConstructor, _) => {
                let regexp = self.dispatch_native(
                    Native::RegExpConstructor,
                    &Value::Undefined,
                    &arguments,
                    current,
                )?;
                self.stack.push(regexp);
            }
            Value::Native(Native::PromiseConstructor, _) => {
                let promise = self.construct_promise(&arguments, current)?;
                self.stack.push(promise);
            }
            Value::Native(Native::AbortControllerConstructor, _) => {
                self.stack.push(abort_controller());
            }
            // `Buffer` is both callable and constructible in Node. Keep construction on the
            // same public facade path as `Buffer(...)`; its dispatcher owns the legacy numeric,
            // string, and view conversion rules.
            Value::Native(Native::NodeBufferGlobal, _) => {
                let built = self.dispatch_native(
                    Native::NodeBufferGlobal,
                    &Value::Undefined,
                    &arguments,
                    current,
                )?;
                self.stack.push(built);
            }
            Value::Native(Native::TextEncoderConstructor, _) => {
                self.stack.push(text_encoder());
            }
            // A signal comes from a controller or from one of the statics, never from a caller.
            Value::Native(Native::AbortSignalConstructor, _) => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Illegal constructor".to_owned(),
                });
            }
            Value::Native(
                kind @ (Native::SetConstructor
                | Native::MapConstructor
                | Native::WeakSetConstructor
                | Native::WeakMapConstructor),
                _,
            ) => {
                let collection = self.construct_collection(kind, &arguments, current)?;
                self.stack.push(collection);
            }
            Value::Native(
                native @ (Native::DisposableStackConstructor
                | Native::AsyncDisposableStackConstructor),
                _,
            ) => {
                let flavour = if native == Native::DisposableStackConstructor {
                    Disposal::Sync
                } else {
                    Disposal::Async
                };
                self.stack.push(new_disposable_stack(flavour));
            }
            Value::Native(Native::WeakRefConstructor, _) => {
                let built = self.construct_weak_ref(&arguments, current)?;
                self.stack.push(built);
            }
            Value::Native(Native::ShadowRealmConstructor, _) => {
                let built = self.construct_shadow_realm()?;
                self.stack.push(built);
            }
            Value::Native(Native::FinalizationRegistryConstructor, _) => {
                let built = self.construct_finalization_registry(&arguments, current)?;
                self.stack.push(built);
            }
            // A bound function's `[[Construct]]` prepends the bound arguments and forwards to
            // the target. The bound *receiver* is dropped: `new` makes its own, which is the
            // whole difference between calling a bound function and constructing one. And when
            // the wrapper is itself the `new.target` -- which is every plain `new f.bind(o)()`
            // -- the target takes its place, because the instance has to inherit from the
            // target's `prototype` and a bound function has none.
            //
            // `is_constructor` already answered for the wrapper by asking the target, so a bound
            // arrow still falls through to the refusal below rather than being run as one.
            Value::Bound(ref bound) if is_constructor(&bound.target) => {
                let mut forwarded = bound.arguments.clone();
                forwarded.extend(arguments);
                let target = bound.target.clone();
                let new_target = if strict_equal(&new_target, &callee) {
                    target.clone()
                } else {
                    new_target
                };
                return self.construct_as(target, forwarded, new_target, current);
            }
            Value::Function(closure) => {
                if !closure_is_constructor(&closure) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "a function is not a constructor".to_owned(),
                    });
                }
                // `new` creates the instance, runs the body with it as `this`, and
                // yields the instance -- unless the body returned an object, which
                // wins. That override is what makes factory constructors work.
                let instance = self.instantiate_for(&closure, &new_target, current)?;
                self.stack.push(Value::Undefined);
                let argc = arguments.len();
                for argument in arguments {
                    self.stack.push(argument);
                }
                self.call_constructing(
                    Rc::clone(&closure),
                    argc as u16,
                    current,
                    instance.clone(),
                    Some(instance),
                    Some(new_target.clone()),
                )?;
            }
            other => {
                return Err(ExecutionError::NotConstructable {
                    instruction: current,
                    value: other.type_name().to_owned(),
                });
            }
        }
        // A native branch leaves exactly the instance it built. A `prototype` that is not an
        // object leaves the intrinsic chain in place, which is what the specification
        // substitutes rather than failing, and a value with no store to keep a link in keeps the
        // one its kind gives it.
        if self.stack.len() == depth + 1 {
            let built = self.stack[depth].clone();
            let mut linked = false;
            if subclassed {
                let named = self.read_property(&new_target, "prototype", current)?;
                if is_reflect_target(&named) {
                    record_prototype(&built, named);
                    linked = true;
                }
            }
            // `GetPrototypeFromConstructor` falls back to *`NewTarget`'s realm's* intrinsic
            // default prototype -- when `prototype` is not an object, and when the constructor is
            // its own `NewTarget` and there is nothing to read. In one realm that fallback is the
            // prototype the value already has implicitly, so nothing is recorded and the fast
            // path is untouched. Across two it is the whole difference: without it
            // `new other.Array()` would be an array of *this* realm, and
            // `new other.Array() instanceof Array` would answer true where the language says
            // false. It is also what the seventy-seven `proto-from-ctor-realm` files test.
            if !linked {
                let realm = value_realm(&new_target);
                if realm != RealmId::PRIMARY {
                    record_prototype(&built, in_realm(prototype_of(&built), realm));
                }
            }
        }
        Ok(())
    }
}
