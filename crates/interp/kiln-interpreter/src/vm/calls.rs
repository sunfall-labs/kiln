use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// Run `body`, turning anything a program could catch into the value it threw.
    ///
    /// The same handler `call_catching` installs, for the cases where what may throw is not a
    /// call but a property read: a getter is ordinary code, and the machinery that has to
    /// *record* its failure rather than propagate it needs the value, not the error.
    pub(crate) fn catching<T>(
        &mut self,
        body: impl FnOnce(&mut Self) -> Result<T, ExecutionError>,
    ) -> Result<T, Value> {
        let installed = self.handlers.len();
        self.handlers.push(Handler {
            target: HOST_HANDLER,
            frame_depth: self.frames.len(),
            stack_height: self.stack.len(),
            native_depth: self.native_depth,
        });
        let outcome = body(self);
        self.handlers.truncate(installed);
        match outcome {
            Ok(value) => Ok(value),
            Err(ExecutionError::CaughtByHost) => {
                Err(self.caught.take().unwrap_or(Value::Undefined))
            }
            Err(error) => match error.as_javascript_error() {
                Some((kind, message)) => {
                    Err(self.construct_error(kind, &[Value::String(message.into())]))
                }
                // A limit this realm imposes rather than a language error. Nothing was written
                // to receive it as a value, so it becomes the plain refusal it is.
                None => Err(Value::String(error.to_string().into())),
            },
        }
    }

    pub(crate) fn call_catching(
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
    pub(crate) fn call_value(
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

    /// Run `body` with `realm` as the running realm, and put the caller's back afterwards.
    ///
    /// Restores on the error path as well as the ordinary one, which is the whole reason it is
    /// a function: a `?` that skipped the restore would leave the *caller* running as another
    /// realm, and every intrinsic it minted after that would belong to the wrong one.
    pub(crate) fn entering<T>(
        &mut self,
        realm: RealmId,
        body: impl FnOnce(&mut Self) -> Result<T, ExecutionError>,
    ) -> Result<T, ExecutionError> {
        if realm == self.current_realm {
            return body(self);
        }
        let caller = std::mem::replace(&mut self.current_realm, realm);
        let produced = body(self);
        // An error the machine raises carries a kind and a sentence, not an object, and stays
        // that way until the frame it unwinds into builds one. In one realm that is free. Across
        // two it is a wrong answer: the object would take its prototype from whichever realm
        // happened to catch it, so `other.Array.prototype.sort.call(a, "nope")` would throw
        // *this* realm's `TypeError` and `e instanceof other.TypeError` would be false. Built
        // here, while the realm that raised it is still the running one.
        let produced = match produced {
            Err(error)
                if !matches!(
                    error,
                    ExecutionError::ThrownAcrossNative | ExecutionError::CaughtByHost
                ) =>
            {
                match error.as_javascript_error() {
                    Some((kind, message)) => {
                        let thrown = self.construct_error(kind, &[Value::String(message.into())]);
                        self.caught = Some(thrown);
                        Err(ExecutionError::ThrownAcrossNative)
                    }
                    None => Err(error),
                }
            }
            other => other,
        };
        self.current_realm = caller;
        produced
    }

    pub(crate) fn call_value_inner(
        &mut self,
        callee: &Value,
        this_value: Value,
        arguments: Vec<Value>,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        // Calling a function from another realm enters that realm. Everything the callee mints
        // while it runs -- an intrinsic read off a table, a `TypeError` it throws, the object it
        // constructs -- belongs to the realm the *function* came from, which is what
        // `GetFunctionRealm` and the specification's "throw a TypeError exception" both mean.
        let realm = value_realm(callee);
        self.entering(realm, |vm| {
            vm.call_value_dispatch(callee, this_value, arguments, instruction)
        })
    }

    pub(crate) fn call_value_dispatch(
        &mut self,
        callee: &Value,
        this_value: Value,
        arguments: Vec<Value>,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        match callee {
            Value::Native(native, _) => {
                // The head of every `%TypedArray%.prototype` method, run before anything else
                // here can look at the receiver or the arguments. A branded one then hands the
                // call on to the `Array.prototype` method it shares, so the work below is done
                // once whichever table the program reached it through.
                if let Some((method, brand_only)) = native.typed_array_prototype_method() {
                    self.validate_typed_array(&this_value, method, brand_only)?;
                    if let Native::TypedArrayGeneric(method) = native {
                        // The flag is what tells the shared implementation which of the two
                        // tables the program reached it through, and it is set here as well as
                        // in `invoke_dispatch` because a method reached by `call`, `apply`,
                        // `bind` or `Reflect.apply` arrives on this path instead. Without it,
                        // `%TypedArray%.prototype.map.call(view, f)` built its result the way
                        // `Array.prototype.map` does and answered an ordinary array.
                        let outer = std::mem::replace(&mut self.typed_array_method, true);
                        let dispatched = self.call_value_inner(
                            &array_property(method),
                            this_value,
                            arguments,
                            instruction,
                        );
                        self.typed_array_method = outer;
                        return dispatched;
                    }
                }
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
                // The same tail `invoke_dispatch` gives a native: a view that reached a copying
                // method gets a view of its own kind back, and one that reached an in-place
                // method gets its own elements rearranged rather than a snapshot's. Both paths
                // reach one `%TypedArray%.prototype` implementation, so both owe it the same
                // ending; this one used to drop the receiver's kind on the floor, which is why
                // `%TypedArray%.prototype.slice.call(view)` answered an ordinary array.
                let original = this_value.clone();
                let this_value = self.as_array_like(callee, this_value, instruction)?;
                let produced =
                    self.dispatch_native(*native, &this_value, &arguments, instruction)?;
                self.typed_result(
                    &original,
                    produced,
                    native.rearranges_in_place(),
                    native.returns_a_new_array(),
                    instruction,
                )
            }
            // `resolve` and `reject` from a promise executor. Calling one settles its promise
            // and evaluates to undefined, however many times the program calls it.
            Value::Settler(promise, rejects) => {
                let value = arguments.first().cloned().unwrap_or(Value::Undefined);
                let promise = Rc::clone(promise);
                if *rejects {
                    if matches!(value, Value::Native(Native::ReturnSignal, _)) {
                        self.resolve_promise(&promise, Value::Undefined);
                    } else {
                        self.settle(&promise, &Settled::Rejected(value));
                    }
                } else {
                    self.resolve_promise(&promise, value);
                }
                Ok(Value::Undefined)
            }
            // The executor a `NewPromiseCapability` hands to a constructor: it keeps the pair it
            // is called with. A constructor that calls it twice is a `TypeError` -- the pair it
            // already recorded is the one the caller is about to use, and replacing it would
            // leave the promise settled by functions nobody holds.
            Value::Capability(record) => {
                let mut resolve = record.resolve.borrow_mut();
                let mut reject = record.reject.borrow_mut();
                if !matches!(*resolve, Value::Undefined) || !matches!(*reject, Value::Undefined) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Promise capability executor has already been invoked".to_owned(),
                    });
                }
                *resolve = arguments.first().cloned().unwrap_or(Value::Undefined);
                *reject = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                Ok(Value::Undefined)
            }
            // A `Promise.all` Resolve Element Function and its `allSettled`/`any` counterparts.
            //
            // `alreadyCalled` is the first step and it is not an optimisation: a thenable may
            // call its `onFulfilled` twice -- test262 has one that does exactly that -- and the
            // second call must not fill the slot again or decrement the count a second time,
            // which would resolve the whole combinator early with a half-filled list.
            Value::Element(element) => {
                let element = Rc::clone(element);
                if element.already_called.get() {
                    return Ok(Value::Undefined);
                }
                element.already_called.set(true);
                let argument = arguments.first().cloned().unwrap_or(Value::Undefined);
                self.settle_combinator_element(&element, argument, instruction)
            }
            // A callback method reaches here when a built-in drives another built-in -- a spread
            // over a generator, for instance. `invoke` is the one place that knows how to run
            // each kind, so this defers to it rather than repeating the dispatch.
            Value::Callback(_, _) => {
                let this_value = self.as_array_like(callee, this_value, instruction)?;
                Ok(self
                    .invoke(callee.clone(), this_value, arguments, instruction)?
                    .unwrap_or(Value::Undefined))
            }
            // A proxied function runs its `apply` trap, or the target when there is none.
            Value::Proxy(proxy) => {
                let proxy = Rc::clone(proxy);
                let inner = proxy.target.clone();
                // A proxy has a `[[Call]]` method only when its target has one. Running the
                // trap regardless made `new Proxy({}, {apply(){}})()` a call where the language
                // says the proxy is not a function -- and a handler could therefore make an
                // ordinary object callable, which no other value can do.
                if !is_callable(&inner) {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: callee.type_name(),
                    });
                }
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
            // Wrapping is the opposite of transparent: the receiver is dropped, every argument
            // is re-wrapped into the target's realm and the result is re-wrapped back out.
            Value::Wrapped(wrapped) => {
                let wrapped = Rc::clone(wrapped);
                self.call_wrapped(&wrapped, arguments, instruction)
            }
            Value::CommonJsRequire(require) => {
                let specifier = arguments
                    .first()
                    .map(Value::to_display_string)
                    .unwrap_or_default();
                if let Some(loader) = self.commonjs_loader
                    && require.importer().is_some()
                {
                    return match unsafe { (&mut *loader).load(require, &specifier, instruction) } {
                        Ok(value) => Ok(value),
                        Err(ExecutionError::ModuleNotFound { message, .. }) => {
                            self.throw_error(ErrorKind::Error, message, instruction)?;
                            Ok(Value::Undefined)
                        }
                        Err(ExecutionError::Uncaught { .. }) => {
                            let value = unsafe { (&mut *loader).take_thrown_value() };
                            if let Some(value) = value {
                                self.caught = Some(value);
                                Err(ExecutionError::ThrownAcrossNative)
                            } else {
                                Err(ExecutionError::Host {
                                    instruction,
                                    message: "CommonJS target failed without a thrown value"
                                        .to_owned(),
                                })
                            }
                        }
                        Err(error) => Err(error),
                    };
                }
                if require.target_cell(&specifier).is_some() {
                    if let Some(loader) = self.commonjs_loader {
                        return match unsafe {
                            (&mut *loader).load(require, &specifier, instruction)
                        } {
                            Ok(value) => Ok(value),
                            Err(ExecutionError::Uncaught { .. }) => {
                                let value = unsafe { (&mut *loader).take_thrown_value() };
                                if let Some(value) = value {
                                    self.caught = Some(value);
                                    Err(ExecutionError::ThrownAcrossNative)
                                } else {
                                    Err(ExecutionError::Host {
                                        instruction,
                                        message: "CommonJS target failed without a thrown value"
                                            .to_owned(),
                                    })
                                }
                            }
                            Err(error) => Err(error),
                        };
                    }
                    if let Some(target) = require.get(&specifier) {
                        return Ok(target);
                    }
                }
                self.throw_error(
                    ErrorKind::Error,
                    format!("Cannot find module '{specifier}'"),
                    instruction,
                )?;
                Ok(Value::Undefined)
            }
            Value::CommonJsResolve(require) => {
                let specifier = arguments
                    .first()
                    .map(Value::to_display_string)
                    .unwrap_or_default();
                if let Some(target) = require.resolve(&specifier) {
                    return Ok(Value::String(target.into()));
                }
                if kiln_builtins::is_builtin_specifier(&specifier) {
                    return Ok(Value::String(specifier.into()));
                }
                self.throw_error(
                    ErrorKind::Error,
                    format!("Cannot find module '{specifier}'"),
                    instruction,
                )?;
                return Ok(Value::Undefined);
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
}
