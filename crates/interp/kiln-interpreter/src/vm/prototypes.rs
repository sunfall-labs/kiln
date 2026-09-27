use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    pub(crate) fn global(&self, slot: usize) -> Result<Value, ExecutionError> {
        let locals = self.module_locals();
        locals
            .get(slot)
            .cloned()
            .ok_or_else(|| ExecutionError::GlobalOutOfBounds {
                index: slot,
                available: locals.len(),
                function: self
                    .function
                    .as_ref()
                    .and_then(|closure| closure.name.as_deref())
                    .map(str::to_owned),
            })
    }

    pub(crate) fn set_global(&mut self, slot: usize, value: Value) -> Result<(), ExecutionError> {
        let locals = match self.frames.get_mut(self.global_base) {
            Some(module) => &mut module.locals,
            None => &mut self.locals,
        };
        let available = locals.len();
        *locals
            .get_mut(slot)
            .ok_or_else(|| ExecutionError::GlobalOutOfBounds {
                index: slot,
                available,
                function: self
                    .function
                    .as_ref()
                    .and_then(|closure| closure.name.as_deref())
                    .map(str::to_owned),
            })? = value;
        Ok(())
    }

    /// Read a module-scope slot from wherever the module body's locals currently live.
    ///
    /// At module level that is `self.locals`; inside any call it is the bottom frame's, because
    /// the module body is the bottom of the call stack. Going through the frame rather than
    /// copying module locals into each callee is what makes a module binding shared: an
    /// assignment from inside a function has to be visible to the next call that reads it.
    pub(crate) fn module_locals(&self) -> &[Value] {
        match self.frames.get(self.global_base) {
            Some(module) => &module.locals,
            None => &self.locals,
        }
    }

    /// `[[GetPrototypeOf]]`, including a proxy's `getPrototypeOf` trap and its invariant.
    /// Prototype walks must use this operation rather than reading the compact representation's
    /// target link, or `for-in`, `instanceof`, and `isPrototypeOf` can disagree with reflection.
    pub(crate) fn prototype_for(
        &mut self,
        target: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if matches!(target, Value::Proxy(_)) {
            self.proxy_get_prototype_of(target, instruction)
        } else {
            Ok(prototype_of(target))
        }
    }

    /// The machine-aware `OrdinaryHasInstance` walk.  A proxy in the value chain contributes its
    /// `getPrototypeOf` trap, while a proxy constructor contributes its `get` trap for
    /// `prototype`; neither may be replaced with the target's private links.
    pub(crate) fn instance_of_value(
        &mut self,
        value: &Value,
        constructor: &Value,
        error_prototype: Option<&Value>,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        // `InstanceofOperator` begins by asking the right-hand side how it wants the question
        // answered. `Symbol.hasInstance` replaces the prototype walk outright -- it is how a
        // program writes a brand check of its own, and how `Array[Symbol.hasInstance]` would be
        // spelled if it were written in JavaScript. It was never consulted, so a class that
        // defined one was answered by the very walk it had replaced.
        //
        // The two refusals around it are the other half of the same operation, and both were
        // answered `false` instead: a right-hand side that is not an object has no `[[Prototype]]`
        // to compare against, and one that is not callable has no `prototype` to compare *to*.
        // `x instanceof 3` is a program that means something impossible, and saying "no" to it is
        // a plausible answer to a question that was never asked.
        if !is_reflect_target(constructor) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Right-hand side of 'instanceof' is not an object".to_owned(),
            });
        }
        let handler = self.read_property(
            constructor,
            WellKnown::HasInstance.property_key(),
            instruction,
        )?;
        if !handler.is_nullish() {
            if !is_callable(&handler) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Symbol.hasInstance is not a function".to_owned(),
                });
            }
            let answered = self.call_value(
                &handler,
                constructor.clone(),
                vec![value.clone()],
                instruction,
            )?;
            return Ok(answered.is_truthy());
        }
        // `InstanceofOperator` step 5 refuses a non-callable right-hand side. `OrdinaryHasInstance`
        // does *not* -- it answers false -- and the difference is observable now that the method
        // is reachable: `x instanceof 3` throws, while calling the method with a non-callable
        // receiver returns false.
        if !is_callable(constructor) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Right-hand side of 'instanceof' is not callable".to_owned(),
            });
        }
        self.ordinary_has_instance(value, constructor, error_prototype, instruction)
    }

    /// `OrdinaryHasInstance(C, O)`: the prototype walk, with no `@@hasInstance` read of its own.
    ///
    /// Split out from [`Vm::instance_of_value`] because `Function.prototype[@@hasInstance]` is now
    /// a real property, so the operator's first step *finds* it for any ordinary function. If that
    /// method re-entered the operator it would read the same property and call itself for ever;
    /// what the specification says it performs is this walk, which is therefore what it calls.
    pub(crate) fn ordinary_has_instance(
        &mut self,
        value: &Value,
        constructor: &Value,
        error_prototype: Option<&Value>,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        // Step 1 of `OrdinaryHasInstance`, and an answer rather than a refusal: the operator
        // has already refused a non-callable right-hand side by the time it gets here, but the
        // method a program calls directly has not, and the specification says it answers false.
        if !is_callable(constructor) {
            return Ok(false);
        }
        // `Iterator` is the one built-in whose instances are told apart by a marker rather than
        // by a prototype the realm has as an object: a generator, an array iterator and a helper
        // all answer to it, and none of their chains reaches an object this walk could compare.
        // A value carrying no marker still falls through to the ordinary walk below, which is
        // what answers for an iterator a program wrote by extending `Iterator` itself.
        if matches!(constructor, Value::Native(Native::IteratorConstructor, _))
            && match value {
                Value::Generator(_) => true,
                Value::Object(object) => {
                    let object = object.borrow();
                    object.get_own("@@iterated").is_some()
                        || object.get_own(HELPER_SOURCE).is_some()
                }
                _ => false,
            }
        {
            return Ok(true);
        }
        let prototype = match constructor {
            Value::Proxy(_) => self.read_computed(
                constructor,
                &Value::String("prototype".to_owned().into()),
                instruction,
            )?,
            Value::Function(closure) => match closure.prototype.borrow().clone() {
                Some(prototype) => prototype,
                None => return Ok(false),
            },
            // Derived from the constructor rather than handed in. It used to be a parameter, and
            // the caller that could not supply one -- `Function.prototype[@@hasInstance]`, which
            // is reached with no operator context at all -- answered false for every error in the
            // realm. That is `e instanceof TypeError` being false, which test262's own
            // `assert.throws` depends on, so it failed hundreds of tests that had nothing to do
            // with `instanceof`. The realm can always answer this question; the caller cannot.
            Value::Native(Native::Error(kind), _) => {
                let kind = *kind;
                let _ = error_prototype;
                self.error_prototype(kind)
            }
            Value::Bound(bound) => {
                // `OrdinaryHasInstance` unwraps a bound function and asks about its target, and
                // it asks *this* operation rather than the operator: the specification does not
                // re-read `@@hasInstance` off the target.
                return self.ordinary_has_instance(
                    value,
                    &bound.target,
                    error_prototype,
                    instruction,
                );
            }
            // A Buffer's chain reaches `Buffer.prototype` once the bootstrap has assigned it;
            // before that the brand is the answer.
            Value::Native(Native::NodeBufferGlobal, _) => match buffer_prototype() {
                Some(prototype) => prototype,
                None => {
                    return Ok(matches!(value, Value::TypedArray(view) if is_buffer_view(view)));
                }
            },
            Value::Native(native, _) => match native.property("prototype") {
                Some(prototype) => prototype,
                None => return Ok(false),
            },
            _ => return Ok(false),
        };
        if !is_an_object(&prototype) {
            return Ok(false);
        }
        let mut current = self.prototype_for(value, instruction)?;
        for _ in 0..MAX_PROTOTYPE_DEPTH {
            if matches!(current, Value::Null | Value::Undefined) {
                return Ok(false);
            }
            if strict_equal(&current, &prototype) {
                return Ok(true);
            }
            current = self.prototype_for(&current, instruction)?;
        }
        Ok(false)
    }

    pub(crate) fn is_prototype_of(
        &mut self,
        target: &Value,
        prototype: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let mut current = self.prototype_for(target, instruction)?;
        for _ in 0..MAX_PROTOTYPE_DEPTH {
            if matches!(current, Value::Null | Value::Undefined) {
                return Ok(Value::Boolean(false));
            }
            if strict_equal(&current, prototype) {
                return Ok(Value::Boolean(true));
            }
            current = self.prototype_for(&current, instruction)?;
        }
        Ok(Value::Boolean(false))
    }

    /// Bytecode of the function currently executing.
    /// The instruction stream of the running frame, borrowed from `program` rather than `self`.
    ///
    /// Taking the `Rc` by value is what lets the interpreter loop hold an instruction across the
    /// `&mut self` calls that execute it: the borrow is on a local clone, not on the machine.
    pub(crate) fn code<'a>(
        program: &'a Rc<Program>,
        function: Option<&Rc<Closure>>,
    ) -> &'a [Instruction] {
        match function {
            None => &program.code,
            // Validated when the closure was created; a call never installs a bad index.
            Some(closure) => &program.functions[closure.function as usize].code,
        }
    }

    /// Run `parent` (on top of the stack, under `argc` arguments) against the current `this`.
    /// The keys `for...in` visits: own enumerable string keys, then the prototype's, and so on
    /// up the chain.
    ///
    /// This read own properties only, on the reasoning that nothing could make an inherited
    /// property enumerable -- but `F.prototype.x = 1` is an ordinary assignment and creates
    /// exactly that, and an object literal used as a prototype is nothing else. So
    /// `for (k in new F())` saw only the instance's own fields.
    ///
    /// A key already seen shadows anything further up the chain whether or not it was
    /// enumerated, which is why the visited set takes *every* own key while only enumerable ones
    /// are yielded: a non-enumerable own property hides an enumerable inherited one, and neither
    /// is visited.
    pub(crate) fn enumerate_for_in(
        &mut self,
        target: &Value,
        current: usize,
    ) -> Result<Vec<Value>, ExecutionError> {
        let mut seen: HashSet<StringUnits> = HashSet::new();
        let mut keys = Vec::new();
        let mut level = target.clone();
        // Bounded like every other prototype walk here: the chain is the program's business, but
        // running out of Rust stack is not something it gets to cause.
        for _ in 0..MAX_PROTOTYPE_DEPTH {
            let own = if matches!(level, Value::Proxy(_)) {
                self.own_keys(&level, current)?
                    .into_iter()
                    .filter(|key| !is_reserved_units(key))
                    .collect()
            } else {
                own_property_keys_units(&self.intrinsics, &level)
            };
            for key in own {
                if !seen.insert(key.clone()) {
                    continue;
                }
                // A value with no attribute record -- an array index, a string index -- is
                // enumerable; only a recorded property can say otherwise.
                let enumerable = if matches!(level, Value::Proxy(_)) {
                    let described =
                        self.proxy_descriptor(&level, &Value::String(key.clone()), current)?;
                    !matches!(described, Value::Undefined)
                        && self
                            .read_property(&described, "enumerable", current)?
                            .is_truthy()
                } else {
                    own_attributes_key(&self.intrinsics, &level, PropertyKeyRef::Units(&key))
                        .is_none_or(|attributes| attributes.enumerable)
                };
                if enumerable {
                    keys.push(Value::String(key));
                }
            }
            level = match self.prototype_for(&level, current)? {
                Value::Undefined | Value::Null => break,
                parent => parent,
            };
        }
        Ok(keys)
    }

    pub(crate) fn super_call(&mut self, argc: u16, current: usize) -> Result<(), ExecutionError> {
        let arguments = self.take_call_arguments(argc, current)?;
        self.super_call_with(arguments, current)
    }

    /// `super(...)` with the argument list already in hand.
    ///
    /// Split from [`Vm::super_call`] so a spread -- whose length is known only at run time --
    /// reaches exactly these construction semantics rather than a second copy of them. Getting
    /// that wrong is not a small matter: the inherited `new.target` and the exotic-parent
    /// prototype transfer below are both invisible until a subclass depends on them.
    pub(crate) fn super_call_with(
        &mut self,
        arguments: Vec<Value>,
        current: usize,
    ) -> Result<(), ExecutionError> {
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
            Value::Native(parent, _) => {
                let Some(built) = self.construct_exotic(parent, &arguments, current)? else {
                    return Err(ExecutionError::NotConstructable {
                        instruction: current,
                        value: format!("native {parent:?}"),
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
                    record_prototype(&built, prototype);
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
                    value: other.type_name().to_owned(),
                });
            }
        }
        Ok(())
    }

    /// The prototype for one error kind, created on first use.
    ///
    /// A subclass's prototype chains to `Error.prototype`, so `e instanceof Error` holds for
    /// every kind and `toString` is inherited rather than copied.
    pub(crate) fn error_prototype(&mut self, kind: ErrorKind) -> Value {
        self.error_prototype_in(kind, self.current_realm)
    }

    /// `realm`'s `TypeError.prototype` and its siblings, built on first use.
    ///
    /// Per realm rather than per execution, because `e instanceof TypeError` compares it by
    /// identity: an error thrown by realm 1's built-in is an instance of *its* `TypeError` and
    /// not of the one the primary realm hands out, and one shared object would make both
    /// answers wrong at once.
    pub(crate) fn error_prototype_in(&mut self, kind: ErrorKind, realm: RealmId) -> Value {
        self.intrinsics.error_prototype(kind, realm)
    }
}
