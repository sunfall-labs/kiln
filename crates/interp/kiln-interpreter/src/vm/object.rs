use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// Read a property, calling a getter when the slot holds one.
    ///
    /// Separate from [`Vm::property`], which answers with the slot's contents, because reading an
    /// accessor is a *call* and only the machine can make one. Every program-visible read goes
    /// through this; the VM's own reads of reserved slots do not need to.
    /// One property of a proxy described, through its trap or through its target.
    pub(crate) fn proxy_descriptor(
        &mut self,
        target: &Value,
        key: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Proxy(proxy) = target else {
            return self.dispatch_native(
                Native::ObjectGetOwnPropertyDescriptor,
                &Value::Undefined,
                &[target.clone(), key.clone()],
                instruction,
            );
        };
        let proxy = Rc::clone(proxy);
        let inner = proxy.target.clone();
        let Some(trap) = self.proxy_trap(&proxy, "getOwnPropertyDescriptor", instruction)? else {
            return self.proxy_descriptor(&inner, key, instruction);
        };
        let handler = proxy.handler.clone();
        let list = vec![inner.clone(), self.trap_key(key)];
        let answered = self.call_value(&trap, handler, list, instruction)?;
        if !is_an_object(&answered) && !matches!(answered, Value::Undefined) {
            return Err(Self::proxy_invariant(
                "getOwnPropertyDescriptor must report an object or undefined",
            ));
        }
        let held = self.own_state(&inner, key, instruction)?;
        let extensible = self.target_extensible(&inner, instruction)?;
        if matches!(answered, Value::Undefined) {
            // Denying a property the target really has is only allowed if the target could
            // still lose it: a non-configurable one it cannot, and a non-extensible target
            // cannot gain the one it is being told it has instead.
            match &held {
                Some((_, attributes)) if !attributes.configurable => {
                    return Err(Self::proxy_invariant(
                        "getOwnPropertyDescriptor cannot report a non-configurable property of \
                         the target as absent",
                    ));
                }
                Some(_) if !extensible => {
                    return Err(Self::proxy_invariant(
                        "getOwnPropertyDescriptor cannot report a property of a non-extensible \
                         target as absent",
                    ));
                }
                _ => {}
            }
            return Ok(Value::Undefined);
        }
        if held.is_none() && !extensible {
            return Err(Self::proxy_invariant(
                "getOwnPropertyDescriptor cannot report a property a non-extensible target does \
                 not have",
            ));
        }
        let mut described = self.read_descriptor(&answered, instruction)?;
        // `FromPropertyDescriptor` completes the two boolean axes before this result reaches a
        // caller. In particular, `{ value: 1 }` means configurable/enumerable/writable false,
        // not three unconstrained fields. The proxy invariant checks must see those defaults too:
        // a configurable target cannot be reported as non-configurable, and a writable locked
        // target cannot be reported as non-writable merely by omitting `writable`.
        described.configurable = Some(described.configurable.unwrap_or(false));
        described.enumerable = Some(described.enumerable.unwrap_or(false));
        if described.is_data() {
            described.writable = Some(described.writable.unwrap_or(false));
        }
        match &held {
            None if !extensible => {
                return Err(Self::proxy_invariant(
                    "getOwnPropertyDescriptor cannot report a property a non-extensible target \
                     does not have",
                ));
            }
            None if described.configurable == Some(false) => {
                return Err(Self::proxy_invariant(
                    "getOwnPropertyDescriptor cannot report a non-configurable property the \
                     target does not have",
                ));
            }
            None => {}
            Some((property, attributes)) => {
                if !Self::descriptor_is_compatible(&described, property, *attributes) {
                    return Err(Self::proxy_invariant(
                        "getOwnPropertyDescriptor cannot report a descriptor incompatible with \
                         the target",
                    ));
                }
                if described.configurable == Some(false) && attributes.configurable {
                    return Err(Self::proxy_invariant(
                        "getOwnPropertyDescriptor cannot report a configurable property of the \
                         target as non-configurable",
                    ));
                }
                if matches!(property, Property::Value(_))
                    && !attributes.configurable
                    && attributes.writable
                    && described.writable == Some(false)
                {
                    return Err(Self::proxy_invariant(
                        "getOwnPropertyDescriptor cannot report a writable property of the \
                         target as non-writable",
                    ));
                }
            }
        }
        // Completed, which is what `[[GetOwnProperty]]` answers with: data and accessor results
        // carry their complete kind-specific fields, while a generic descriptor is completed as
        // a data descriptor with an undefined value.
        Ok(Self::completed_descriptor(&described))
    }

    /// Run a mutating array method against a proxy, writing the result back through its traps.
    ///
    /// The elements are read out, the method runs on that array, and every element plus `length`
    /// goes back through `set`. Not the specification's element-by-element algorithm -- that one
    /// interleaves reads and writes in an order a handler can observe -- but the same end state,
    /// which is what a copy-on-write handler such as immer's is watching for.
    pub(crate) fn mutate_through_proxy(
        &mut self,
        native: Native,
        target: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let length = bounded_length(
            self.proxy_get(
                target,
                &Value::String("length".to_owned().into()),
                instruction,
            )?
            .as_number(),
        )?;
        let mut items = Vec::with_capacity(length);
        for index in 0..length {
            items.push(self.proxy_get(target, &Value::Number(index as f64), instruction)?);
        }
        let scratch = Value::Array(ArrayData::new(items));
        let answer = self.dispatch_native(native, &scratch, arguments, instruction)?;
        let Value::Array(updated) = &scratch else {
            unreachable!("just built as an array");
        };
        let updated = updated.borrow().clone();
        for (index, item) in updated.iter().enumerate() {
            self.set_computed(
                target,
                &Value::Number(index as f64),
                item.clone(),
                instruction,
            )?;
        }
        // The length last, so a handler watching it sees a consistent array when it fires.
        self.set_computed(
            target,
            &Value::String("length".to_owned().into()),
            Value::Number(updated.len() as f64),
            instruction,
        )?;
        Ok(answer)
    }

    /// Put the result of a generic array method, borrowed onto a typed array, back through it.
    ///
    /// One that rearranges in place writes back through the view and answers with it --
    /// `Array.prototype.reverse.call(view)` reverses the view. Everything else passes through:
    /// a borrowed `map` or `slice` answers an ordinary array, as `ArraySpeciesCreate` says it
    /// must for a receiver that is not an array.
    pub(crate) fn typed_result(
        &mut self,
        original: &Value,
        produced: Value,
        in_place: bool,
    ) -> Result<Value, ExecutionError> {
        let Value::TypedArray(view) = original else {
            return Ok(produced);
        };
        let Value::Array(items) = &produced else {
            return Ok(produced);
        };
        if in_place {
            for (index, item) in items.borrow().iter().enumerate() {
                view.set(index, item)?;
            }
            return Ok(original.clone());
        }
        Ok(produced)
    }

    /// `TypedArrayCreateFromConstructor(C, «count»)`, then fill.
    ///
    /// `C` is the `this` of `%TypedArray%.from` and `.of`, which is what makes them inheritable:
    /// a subclass reaches them through its own name and must get its own type back, and
    /// `Int8Array.of.call({}, 1)` must be a `TypeError` rather than an ordinary object with a
    /// `0` property. The intrinsic for `kind` is the fallback for a receiver that is not a
    /// constructor value at all, which is how these are reached internally.
    pub(crate) fn typed_array_through(
        &mut self,
        constructor: &Value,
        kind: TypedKind,
        elements: Vec<Value>,
        mapper: Option<(Value, Value)>,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        // Only an *absent* receiver falls back to the intrinsic, which is how these are reached
        // internally. A receiver the program supplied is held to the specification's rule even
        // when it is a plain object: `Int8Array.of.call({}, 1)` is a `TypeError`, not an Int8Array
        // and not an ordinary object with a `0` property.
        // A view of the right length, filled below -- not a view built *from* the values. The
        // mapper has to run after the view exists, which is the order `%TypedArray%.from`
        // specifies and the order a mapper that detaches the buffer can tell apart.
        if matches!(constructor, Value::Undefined) {
            let out = construct_typed_array(kind, &[Value::Number(elements.len() as f64)])?;
            return self.fill_typed_array_through(out, elements, mapper, instruction);
        }
        if !is_constructor(constructor) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{} is not a constructor", constructor.to_display_string()),
            });
        }
        let count = elements.len();
        let out = self.construct_value(
            constructor.clone(),
            vec![Value::Number(count as f64)],
            constructor.clone(),
            instruction,
        )?;
        let Value::TypedArray(target) = &out else {
            return Err(ExecutionError::TypeErrorMessage {
                message: "a typed array constructor must answer a typed array".to_owned(),
            });
        };
        if target.len() < count {
            return Err(ExecutionError::TypeErrorMessage {
                message: "a typed array constructor answered a view that is too short".to_owned(),
            });
        }
        self.fill_typed_array_through(out, elements, mapper, instruction)
    }

    /// Fill a freshly built view, running the mapper where `%TypedArray%.from` runs it.
    ///
    /// After the view exists, never before: a mapper is the program's own code and may detach
    /// the buffer or resize it, and the specification lets it see a view that is already there.
    /// Mapping first and building afterwards made `from` refuse a result the language accepts.
    pub(crate) fn fill_typed_array_through(
        &mut self,
        out: Value,
        elements: Vec<Value>,
        mapper: Option<(Value, Value)>,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::TypedArray(target) = &out else {
            return Err(ExecutionError::TypeErrorMessage {
                message: "a typed array constructor must answer a typed array".to_owned(),
            });
        };
        let target = Rc::clone(target);
        for (index, item) in elements.into_iter().enumerate() {
            let item = match &mapper {
                Some((function, this_argument)) => self.call_value(
                    function,
                    this_argument.clone(),
                    Vec::from([item, Value::Number(index as f64)]),
                    instruction,
                )?,
                None => item,
            };
            // `Set(target, k, value, true)` converts to the view's element type, and for an
            // object that conversion runs its `valueOf`. The pure store answered zero, so
            // `Uint8Array.of({valueOf: () => 3})` was a view holding nought.
            let item = self.as_element(target.kind, &item, instruction)?;
            target.set(index, &item)?;
        }
        Ok(out)
    }

    /// `SpeciesConstructor(exemplar, %TypedArray%)`, or `None` for "use the default".
    ///
    /// Every step is observable and every step is taken: `constructor` off the exemplar, then
    /// `@@species` off that. `null` and `undefined` both mean the default, which is how a
    /// subclass opts *out* -- `static get [Symbol.species]() { return Int8Array; }` -- and
    /// anything else that is not a constructor is a `TypeError` rather than a silent fallback.
    pub(crate) fn typed_array_species(
        &mut self,
        exemplar: &Value,
        instruction: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        let constructor = self.read_property(exemplar, "constructor", instruction)?;
        let species = match &constructor {
            // An absent `constructor` is the default; anything that is not an object is a
            // `TypeError` rather than a silent fallback.
            Value::Undefined => Value::Undefined,
            other if is_an_object(other) || is_callable(other) => {
                self.read_property(other, WellKnown::Species.property_key(), instruction)?
            }
            other => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!(
                        "{} is not an object and cannot be a constructor",
                        other.to_display_string()
                    ),
                });
            }
        };
        match species {
            Value::Undefined | Value::Null => Ok(None),
            species if is_constructor(&species) => Ok(Some(species)),
            other => Err(ExecutionError::TypeErrorMessage {
                message: format!("{} is not a constructor", other.to_display_string()),
            }),
        }
    }

    /// The key a trap is handed.
    ///
    /// A symbol stays a symbol: a handler that special-cases one -- `if (key === MY_SYMBOL)` --
    /// is the standard way a library hides its own state behind a proxy, and stringifying the
    /// key breaks exactly that. Everything else is a string, as the specification says, so a
    /// numeric index arrives as `"0"`.
    pub(crate) fn trap_key(&self, key: &Value) -> Value {
        match key {
            Value::Symbol(_) | Value::String(_) => key.clone(),
            other => {
                // A symbol that reached here as its reserved spelling is turned back into the
                // symbol. The interpreter stores a symbol key as a string, and handing that
                // string to a trap showed the handler `"@@symbol:1"` where the language shows
                // it the symbol -- so `Object.freeze(proxy)` asked about a name no program can
                // hold. `symbol_of_key` draws the line this depends on: a symbol the program
                // made comes back, the engine's own bookkeeping does not.
                let spelling = other.property_key_lossy();
                match self.symbol_of_key(&spelling) {
                    Some(symbol) => Value::Symbol(symbol),
                    None => Value::String(spelling.into()),
                }
            }
        }
    }

    /// A `TypeError` naming the invariant a handler broke.
    ///
    /// Every one of these says the same thing in a different place: a trap may lie about
    /// anything the target has not already promised, and nothing it has. The specification calls
    /// them the essential internal method invariants, and they are what makes a proxy safe to
    /// hand to code that has not been told it is one.
    pub(crate) fn proxy_invariant(what: &str) -> ExecutionError {
        ExecutionError::TypeErrorMessage {
            message: format!("proxy {what}"),
        }
    }

    /// `target.[[GetOwnProperty]](key)`: what the target really holds, for a check against what
    /// the trap claimed.
    ///
    /// A proxy may itself be a proxy's target, and then the question is another trap rather than
    /// a store -- so this asks the operation, not the object.
    pub(crate) fn own_state(
        &mut self,
        target: &Value,
        key: &Value,
        instruction: usize,
    ) -> Result<Option<(Property, Attributes)>, ExecutionError> {
        let units = key.property_key_units_checked()?;
        self.own_state_key(
            target,
            key,
            PropertyKeyRef::from_value(key, &units),
            instruction,
        )
    }

    pub(crate) fn own_state_key(
        &mut self,
        target: &Value,
        key_value: &Value,
        key: PropertyKeyRef<'_>,
        instruction: usize,
    ) -> Result<Option<(Property, Attributes)>, ExecutionError> {
        if matches!(target, Value::Proxy(_)) {
            let described = self.proxy_descriptor(target, key_value, instruction)?;
            if !is_an_object(&described) {
                return Ok(None);
            }
            let descriptor = self.read_descriptor(&described, instruction)?;
            let attributes = Attributes {
                writable: descriptor.writable.unwrap_or(false),
                enumerable: descriptor.enumerable.unwrap_or(false),
                configurable: descriptor.configurable.unwrap_or(false),
            };
            let property = if descriptor.is_accessor() {
                Property::Accessor {
                    get: descriptor
                        .get
                        .filter(|half| !matches!(half, Value::Undefined)),
                    set: descriptor
                        .set
                        .filter(|half| !matches!(half, Value::Undefined)),
                }
            } else {
                Property::Value(descriptor.value.unwrap_or(Value::Undefined))
            };
            return Ok(Some((property, attributes)));
        }
        Ok(own_property_state_key(&self.intrinsics, target, key))
    }

    /// `target.[[IsExtensible]]()`, which is a trap when the target is itself a proxy.
    pub(crate) fn target_extensible(
        &mut self,
        target: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        if matches!(target, Value::Proxy(_)) {
            return self.proxy_is_extensible(target, instruction);
        }
        Ok(is_extensible_target(&self.intrinsics, target))
    }

    /// `target.[[GetPrototypeOf]]()`, which is a trap when the target is itself a proxy.
    pub(crate) fn target_prototype(
        &mut self,
        target: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if matches!(target, Value::Proxy(_)) {
            return self.proxy_get_prototype_of(target, instruction);
        }
        Ok(prototype_of(target))
    }

    /// `proxy.[[IsExtensible]]()`.
    ///
    /// The trap may not disagree with the target at all -- unlike almost every other one, there
    /// is no room here for a handler to invent an answer, because extensibility is what every
    /// other invariant is checked against.
    pub(crate) fn proxy_is_extensible(
        &mut self,
        target: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        let Value::Proxy(proxy) = target else {
            return Ok(is_extensible_target(&self.intrinsics, target));
        };
        let proxy = Rc::clone(proxy);
        let inner = proxy.target.clone();
        let Some(trap) = self.proxy_trap(&proxy, "isExtensible", instruction)? else {
            return self.target_extensible(&inner, instruction);
        };
        let handler = proxy.handler.clone();
        let answered = self
            .call_value(&trap, handler, vec![inner.clone()], instruction)?
            .is_truthy();
        if answered != self.target_extensible(&inner, instruction)? {
            return Err(Self::proxy_invariant(
                "isExtensible must report what the target reports",
            ));
        }
        Ok(answered)
    }

    /// `proxy.[[PreventExtensions]]()`.
    pub(crate) fn proxy_prevent_extensions(
        &mut self,
        target: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        let Value::Proxy(proxy) = target else {
            return Ok(true);
        };
        let proxy = Rc::clone(proxy);
        let inner = proxy.target.clone();
        let Some(trap) = self.proxy_trap(&proxy, "preventExtensions", instruction)? else {
            return Ok(self
                .dispatch_native(
                    Native::ReflectPreventExtensions,
                    &Value::Undefined,
                    &[inner],
                    instruction,
                )?
                .is_truthy());
        };
        let handler = proxy.handler.clone();
        let answered = self
            .call_value(&trap, handler, vec![inner.clone()], instruction)?
            .is_truthy();
        // A handler may refuse, and then the answer is simply false. What it may not do is
        // report success while the target is still open: every later invariant would then be
        // checked against a promise nothing kept.
        if answered && self.target_extensible(&inner, instruction)? {
            return Err(Self::proxy_invariant(
                "preventExtensions cannot report success while the target is still extensible",
            ));
        }
        Ok(answered)
    }

    /// `proxy.[[GetPrototypeOf]]()`.
    pub(crate) fn proxy_get_prototype_of(
        &mut self,
        target: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Proxy(proxy) = target else {
            return Ok(prototype_of(target));
        };
        let proxy = Rc::clone(proxy);
        let inner = proxy.target.clone();
        let Some(trap) = self.proxy_trap(&proxy, "getPrototypeOf", instruction)? else {
            return self.target_prototype(&inner, instruction);
        };
        let handler = proxy.handler.clone();
        let answered = self.call_value(&trap, handler, vec![inner.clone()], instruction)?;
        if !is_an_object(&answered) && !matches!(answered, Value::Null) {
            return Err(Self::proxy_invariant(
                "getPrototypeOf must report an object or null",
            ));
        }
        // An extensible target's prototype can still be changed, so a trap may answer anything.
        // A non-extensible one's cannot, and the answer has to be the truth.
        if self.target_extensible(&inner, instruction)? {
            return Ok(answered);
        }
        let actual = self.target_prototype(&inner, instruction)?;
        if !same_value(&answered, &actual) {
            return Err(Self::proxy_invariant(
                "getPrototypeOf must report the target's prototype once the target is not \
                 extensible",
            ));
        }
        Ok(answered)
    }

    /// `proxy.[[SetPrototypeOf]](value)`.
    pub(crate) fn proxy_set_prototype_of(
        &mut self,
        target: &Value,
        prototype: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        let Value::Proxy(proxy) = target else {
            return Ok(true);
        };
        let proxy = Rc::clone(proxy);
        let inner = proxy.target.clone();
        let Some(trap) = self.proxy_trap(&proxy, "setPrototypeOf", instruction)? else {
            return Ok(self
                .dispatch_native(
                    Native::ReflectSetPrototypeOf,
                    &Value::Undefined,
                    &[inner, prototype.clone()],
                    instruction,
                )?
                .is_truthy());
        };
        let handler = proxy.handler.clone();
        let answered = self
            .call_value(
                &trap,
                handler,
                vec![inner.clone(), prototype.clone()],
                instruction,
            )?
            .is_truthy();
        if !answered {
            return Ok(false);
        }
        if self.target_extensible(&inner, instruction)? {
            return Ok(true);
        }
        let actual = self.target_prototype(&inner, instruction)?;
        if !same_value(prototype, &actual) {
            return Err(Self::proxy_invariant(
                "setPrototypeOf cannot report success for a prototype the non-extensible target \
                 does not have",
            ));
        }
        Ok(true)
    }

    /// Apply an integrity level to a proxy through its internal methods.  The target is not
    /// visible to this operation: `preventExtensions`, `ownKeys`, `getOwnPropertyDescriptor`
    /// and `defineProperty` are all observable traps, and each one carries its own invariant.
    pub(crate) fn proxy_integrity_level(
        &mut self,
        target: &Value,
        freeze: bool,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if !self.proxy_prevent_extensions(target, instruction)? {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Object integrity level could not close the object".to_owned(),
            });
        }
        let keys = self.own_keys(target, instruction)?;
        for key in keys {
            let mut data = ObjectData::default();
            if freeze {
                let described =
                    self.proxy_descriptor(target, &Value::String(key.clone()), instruction)?;
                if matches!(described, Value::Undefined) {
                    continue;
                }
                let described = self.read_descriptor(&described, instruction)?;
                if described.is_accessor() {
                    data.set("get", described.get.unwrap_or(Value::Undefined));
                    data.set("set", described.set.unwrap_or(Value::Undefined));
                } else {
                    data.set("value", described.value.unwrap_or(Value::Undefined));
                    data.set("writable", Value::Boolean(false));
                }
            }
            data.set("configurable", Value::Boolean(false));
            let descriptor = Value::Object(Rc::new(RefCell::new(data)));
            self.dispatch_native(
                Native::ObjectDefineProperty,
                &Value::Undefined,
                &[target.clone(), Value::String(key), descriptor],
                instruction,
            )?;
        }
        Ok(target.clone())
    }

    /// Test a proxy's integrity level using the proxy's own answers.  In particular, a target
    /// that is already closed does not make a lying descriptor trap acceptable: the descriptor
    /// returned through `getOwnPropertyDescriptor` is what `TestIntegrityLevel` observes.
    pub(crate) fn proxy_is_integrity_level(
        &mut self,
        target: &Value,
        frozen: bool,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if self.proxy_is_extensible(target, instruction)? {
            return Ok(Value::Boolean(false));
        }
        for key in self.own_keys(target, instruction)? {
            let described = self.proxy_descriptor(target, &Value::String(key), instruction)?;
            if matches!(described, Value::Undefined) {
                continue;
            }
            let described = self.read_descriptor(&described, instruction)?;
            if described.configurable != Some(false)
                || (frozen && !described.is_accessor() && described.writable != Some(false))
            {
                return Ok(Value::Boolean(false));
            }
        }
        Ok(Value::Boolean(true))
    }

    /// The own enumerable keys of a proxy paired with what reading each one answers.
    ///
    /// A proxy has no property store to walk, so the only way to ask what it holds is the three
    /// traps the specification uses: `ownKeys` for the names, `getOwnPropertyDescriptor` for
    /// which of them are enumerable, and `get` for the value. Every one of those is observable,
    /// and a handler that counts its calls can tell the difference -- so this asks the proxy
    /// rather than reaching past it to the target, even where the handler has no trap of its own.
    ///
    /// Shared by `Object.assign` and object spread because the two copy the same set: the
    /// spread was reading the store directly, found none, and produced `{}` from a proxy that
    /// `Object.assign` beside it copied correctly.
    /// Walk a proxy source's own enumerable properties, handing each to `visit` as soon as it is
    /// read.
    ///
    /// The visit happens inside the walk rather than after it because `Object.assign` writes
    /// through this and the order is observable. `CopyDataProperties` reads and writes one key
    /// at a time -- `[[GetOwnProperty]]`, `[[Get]]`, `Set` -- so a proxy source feeding a proxy
    /// target sees `desc:b, get:b, set:b, desc:a, get:a, set:a`. Collecting every value first and
    /// writing them afterwards produced `desc:b, get:b, desc:a, get:a, set:b, set:a`, which is
    /// the same properties in the same order and still the wrong trace: a handler that decides
    /// what to return from a later `get` based on an earlier `set` observes the difference.
    pub(crate) fn for_each_proxy_own_enumerable(
        &mut self,
        source: &Value,
        instruction: usize,
        mut visit: impl FnMut(&mut Self, StringUnits, Value) -> Result<(), ExecutionError>,
    ) -> Result<(), ExecutionError> {
        for key in self.own_keys(source, instruction)? {
            let descriptor =
                self.proxy_descriptor(source, &Value::String(key.clone()), instruction)?;
            if matches!(descriptor, Value::Undefined)
                || !self
                    .read_property(&descriptor, "enumerable", instruction)?
                    .is_truthy()
            {
                continue;
            }
            // The read is the `get` trap, not the descriptor's `value`: a proxy is free to
            // answer a read with something its descriptor did not mention, and a spread copies
            // what a read produces.
            let value = self.read_property_key(source, PropertyKeyRef::Units(&key), instruction)?;
            visit(self, key, value)?;
        }
        Ok(())
    }

    /// The same walk, collected.  An object spread builds a fresh object with no traps on it, so
    /// nothing can observe when the writes happen relative to the reads.
    pub(crate) fn proxy_own_enumerable(
        &mut self,
        source: &Value,
        instruction: usize,
    ) -> Result<Vec<(StringUnits, Value)>, ExecutionError> {
        let mut copied = Vec::new();
        self.for_each_proxy_own_enumerable(source, instruction, |_, key, value| {
            copied.push((key, value));
            Ok(())
        })?;
        Ok(copied)
    }

    /// What a `set` trap that answered truthily may not have claimed.
    ///
    /// A handler is free to route a write anywhere it likes -- that is the point of the trap --
    /// but not to report success for a write the target itself would have refused and whose
    /// refusal the target has promised to keep.
    pub(crate) fn check_set_invariant(
        &mut self,
        inner: &Value,
        key: &Value,
        value: &Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        match self.own_state(inner, key, instruction)? {
            Some((Property::Value(held), attributes))
                if !attributes.configurable && !attributes.writable =>
            {
                if same_value(value, &held) {
                    Ok(())
                } else {
                    Err(Self::proxy_invariant(
                        "set cannot report a write to a non-writable, non-configurable property \
                         of the target",
                    ))
                }
            }
            Some((Property::Accessor { set: None, .. }, attributes))
                if !attributes.configurable =>
            {
                Err(Self::proxy_invariant(
                    "set cannot report a write through a non-configurable accessor with no setter",
                ))
            }
            _ => Ok(()),
        }
    }

    /// What a `defineProperty` trap that answered truthily may not have claimed.
    ///
    /// Adding to a closed target, or claiming a lock the target does not have, are the two ways
    /// a handler can report a definition the target would not have made -- and both leave a
    /// caller believing something about the target that is not true.
    pub(crate) fn check_define_invariant(
        &mut self,
        inner: &Value,
        key: &Value,
        described: &Descriptor,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let held = self.own_state(inner, key, instruction)?;
        let extensible = self.target_extensible(inner, instruction)?;
        let locking = described.configurable == Some(false);
        let Some((property, attributes)) = held else {
            if !extensible {
                return Err(Self::proxy_invariant(
                    "defineProperty cannot add a property to a non-extensible target",
                ));
            }
            if locking {
                return Err(Self::proxy_invariant(
                    "defineProperty cannot report a non-configurable property the target does \
                     not have",
                ));
            }
            return Ok(());
        };
        if !Self::descriptor_is_compatible(described, &property, attributes) {
            return Err(Self::proxy_invariant(
                "defineProperty cannot report a definition the target would have refused",
            ));
        }
        if locking && attributes.configurable {
            return Err(Self::proxy_invariant(
                "defineProperty cannot lock down a property the target left configurable",
            ));
        }
        // A target property that is non-configurable but still *writable* has not promised its
        // value, so a trap may not report it frozen either.
        if matches!(property, Property::Value(_))
            && !attributes.configurable
            && attributes.writable
            && described.writable == Some(false)
        {
            return Err(Self::proxy_invariant(
                "defineProperty cannot report a writable, non-configurable property of the \
                 target made non-writable",
            ));
        }
        Ok(())
    }

    /// `IsCompatiblePropertyDescriptor`: whether an ordinary object would have accepted this
    /// redefinition of a property it already has.
    ///
    /// A configurable property may be redefined into anything at all, which is what configurable
    /// means. Everything below is what a *non*-configurable one still guarantees: its kind, its
    /// enumerability, and -- once it is also non-writable -- its value.
    pub(crate) fn descriptor_is_compatible(
        described: &Descriptor,
        property: &Property,
        attributes: Attributes,
    ) -> bool {
        if attributes.configurable {
            return true;
        }
        if described.configurable == Some(true) {
            return false;
        }
        if described
            .enumerable
            .is_some_and(|wanted| wanted != attributes.enumerable)
        {
            return false;
        }
        if described.is_generic() {
            return true;
        }
        match property {
            Property::Value(held) => {
                if described.is_accessor() {
                    return false;
                }
                if attributes.writable {
                    return true;
                }
                if described.writable == Some(true) {
                    return false;
                }
                described
                    .value
                    .as_ref()
                    .is_none_or(|wanted| same_value(wanted, held))
            }
            Property::Accessor { get, set } => {
                if described.is_data() {
                    return false;
                }
                let half = |wanted: Option<&Value>, held: Option<&Value>| {
                    wanted
                        .is_none_or(|wanted| same_value(wanted, held.unwrap_or(&Value::Undefined)))
                };
                half(described.get.as_ref(), get.as_ref())
                    && half(described.set.as_ref(), set.as_ref())
            }
        }
    }

    /// `FromPropertyDescriptor`: the fields the record holds, and no others.
    ///
    /// Distinct from [`Self::completed_descriptor`], which fills the absent ones in with their
    /// defaults. A *partial* descriptor is a real thing in the specification -- it is what
    /// `[[Set]]` hands `[[DefineOwnProperty]]` to change a value without touching the flags --
    /// and a handler that is shown a completed one is being told about attributes nobody asked
    /// to change.
    pub(crate) fn descriptor_object(described: &Descriptor) -> Value {
        let mut data = ObjectData::default();
        if let Some(value) = &described.value {
            data.set("value", value.clone());
        }
        if let Some(writable) = described.writable {
            data.set("writable", Value::Boolean(writable));
        }
        if let Some(get) = &described.get {
            data.set("get", get.clone());
        }
        if let Some(set) = &described.set {
            data.set("set", set.clone());
        }
        if let Some(enumerable) = described.enumerable {
            data.set("enumerable", Value::Boolean(enumerable));
        }
        if let Some(configurable) = described.configurable {
            data.set("configurable", Value::Boolean(configurable));
        }
        Value::Object(Rc::new(RefCell::new(data)))
    }

    /// `FromPropertyDescriptor(ToPropertyDescriptor(x))`: every field spelled out.
    pub(crate) fn completed_descriptor(described: &Descriptor) -> Value {
        let mut data = ObjectData::default();
        if described.is_accessor() {
            data.set("get", described.get.clone().unwrap_or(Value::Undefined));
            data.set("set", described.set.clone().unwrap_or(Value::Undefined));
        } else {
            data.set("value", described.value.clone().unwrap_or(Value::Undefined));
            data.set(
                "writable",
                Value::Boolean(described.writable.unwrap_or(false)),
            );
        }
        data.set(
            "enumerable",
            Value::Boolean(described.enumerable.unwrap_or(false)),
        );
        data.set(
            "configurable",
            Value::Boolean(described.configurable.unwrap_or(false)),
        );
        Value::Object(Rc::new(RefCell::new(data)))
    }

    /// The trap `name` names on a proxy's handler, or `None` when the handler does not have one.
    ///
    /// A missing trap means the operation falls through to the target, which is what makes a
    /// handler with one trap a useful thing to write.
    pub(crate) fn proxy_trap(
        &mut self,
        proxy: &Rc<ProxyData>,
        name: &str,
        instruction: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        if proxy.revoked.get() {
            return Err(ExecutionError::RevokedProxy);
        }
        let trap = self.read_property(&proxy.handler, name, instruction)?;
        // `GetMethod`: only `undefined` and `null` mean "no trap". Anything else that is not
        // callable is a handler the program got wrong, and quietly falling through to the target
        // would run the operation the handler was written to intercept.
        if trap.is_nullish() {
            return Ok(None);
        }
        if !is_callable(&trap) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("proxy handler's `{name}` trap is not callable"),
            });
        }
        Ok(Some(trap))
    }

    /// What the *stores* say about one name: the own property, or the nearest one up the chain.
    ///
    /// This is deliberately only the stores. A built-in's tabulated methods are not here, and
    /// neither are the handful of accessors the machine answers itself, so `None` means "nothing
    /// a program put there" rather than "no such property" -- which is the question both the
    /// read path and the write path have to ask before either falls back to one of those
    /// accessors, or they would disagree about which shadows which.
    pub(crate) fn stored_property(&self, target: &Value, name: &str) -> Option<Property> {
        self.stored_property_key(target, PropertyKeyRef::Utf8(name))
    }

    pub(crate) fn stored_property_key(
        &self,
        target: &Value,
        key: PropertyKeyRef<'_>,
    ) -> Option<Property> {
        // Every arm below finds what the value *stores*, and then the same walk finds what a
        // built-in prototype on its chain carries. Splitting that second half across the arms is
        // what left it on objects alone: an accessor defined on `Object.prototype` was reachable
        // through a plain object and through nothing else, though every value in the language
        // inherits from it.
        self.own_stored_property_key(target, key)
            .or_else(|| self.inherited_accessor_key(target, key))
    }

    /// An accessor a built-in prototype above `target` carries.
    ///
    /// Split from the walk itself so that both guards are asked before any work is done. The
    /// first is the common case by an enormous margin -- no program has defined an accessor on
    /// any built-in -- and this sits on the path of `array.map`.
    ///
    /// The second is the one that cost forty-two rows of `built-ins/String` and thirty-six of
    /// `built-ins/TypedArrayConstructors` when it was missing. An own property is not looked up
    /// on a chain, and a value may answer a name without storing it anywhere: a string's
    /// characters and its length, a view's elements, an array's dense elements. The stores were
    /// searched above and hold none of those, so the question has to be put to the value itself
    /// before the chain is walked -- and `own_property_state` is where the language already
    /// answers it, for `getOwnPropertyDescriptor` and now for this. Asking it here rather than
    /// listing the kinds again is the difference between one authority and two that drift.
    ///
    /// Without it, a getter on `Object.prototype` answered for a character of a string and an
    /// element of a view. That is not an exotic shape: installing one is precisely how test262
    /// detects a lookup that should never have reached the prototype at all.
    pub(crate) fn inherited_accessor_key(
        &self,
        target: &Value,
        key: PropertyKeyRef<'_>,
    ) -> Option<Property> {
        if !self.intrinsics.any_accessor() {
            return None;
        }
        if own_property_state_key(&self.intrinsics, target, key).is_some() {
            return None;
        }
        // An exotic `[[Get]]` answers some names itself whether or not it *has* one: a view's
        // `sample["1.1"]`, its `"-0"` and an index past its end are all `undefined` with the
        // chain never consulted. That is the exotic method rather than an absence, so
        // `own_property_state` above answers `None` for them and would let the walk run --
        // which is exactly what `internals/Get/key-is-not-integer.js` installs a getter on
        // `%TypedArray%.prototype` named `"1.1"` to catch.
        if exotic_reserved_name_key(target, key) {
            return None;
        }
        inherited_intrinsic_accessor_key(&self.intrinsics, target, key)
    }

    /// What `target` itself stores under `name` -- its own store and any chain of stores linked
    /// from it, but not the built-in prototype that chain ends at.
    pub(crate) fn own_stored_property_key(
        &self,
        target: &Value,
        key: PropertyKeyRef<'_>,
    ) -> Option<Property> {
        match target {
            Value::Shared(facade) => facade.own_property(key).map(|(property, _)| property),
            Value::Object(object) => object.borrow().lookup_key(key),
            Value::Function(closure) if !key.matches_ascii("prototype") => {
                closure.properties.borrow().lookup_key(key)
            }
            // An array's non-element properties live in an ordinary store with an ordinary
            // chain, so a subclass's accessor is reachable the same way. An *index* may hold an
            // accessor too, either its own or one the chain carries where the array has a hole.
            Value::Array(items) => match array_index_key(key) {
                Some(index) => match array_exotic_element(items, index) {
                    Some(entry) => Some(entry.property),
                    // A dense element answers, and it is in no store: `inherited_accessor` is
                    // what stops the walk, by asking the array whether it owns the name.
                    None if items
                        .borrow()
                        .get(index)
                        .is_some_and(|value| !Self::is_hole(value)) =>
                    {
                        None
                    }
                    None => array_inherited_index_key(&self.intrinsics, items, key),
                },
                None => items.properties.borrow().lookup_key(key),
            },
            // A built-in keeps what a program defined on it in the realm's store, accessors
            // included: `Object.defineProperty(Math, "x", { get })` has to run its getter here
            // rather than read as undefined.
            Value::Bound(bound) => bound.properties.borrow().lookup_key(key),
            Value::Native(_, _) | Value::Callback(_, _) => {
                match self.intrinsics.overridden_key(target, key) {
                    Overridden::Held(property, _) => Some(property),
                    // Deleted, or never touched: either way the chain above may still answer.
                    Overridden::Absent | Overridden::Untouched => None,
                }
            }
            // An exotic object's named properties live in a store beside its own state,
            // accessors included: `Object.defineProperty(sample, "x", { get })` has to run its
            // getter here rather than read as undefined.
            _ => exotic_properties(target)
                .filter(|_| !exotic_reserved_name_key(target, key))
                .and_then(|store| store.borrow().lookup_key(key)),
        }
    }

    /// `target.name`, refusing a binding that is not there yet.
    ///
    /// The dead-zone marker reaches a *property* by exactly one route: a module namespace shows
    /// a binding whose declaration has not run, and the specification makes reading it a
    /// ReferenceError rather than an `undefined`. No other property can hold the marker, so the
    /// check costs one discriminant comparison and is asked once instead of at each of the
    /// reads below.
    pub(crate) fn read_property(
        &mut self,
        target: &Value,
        name: &str,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        self.read_property_key(target, PropertyKeyRef::Utf8(name), instruction)
    }

    pub(crate) fn read_property_key(
        &mut self,
        target: &Value,
        key: PropertyKeyRef<'_>,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let value = self.read_property_inner_key(target, key, instruction)?;
        if matches!(value, Value::Native(Native::Uninitialized, _)) {
            let name = key.owned().to_lossy_string();
            return Err(uninitialized_export_error(&name));
        }
        Ok(value)
    }

    pub(crate) fn read_property_inner_key(
        &mut self,
        target: &Value,
        key: PropertyKeyRef<'_>,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if matches!(target, Value::Proxy(_)) {
            return self.proxy_get(target, &Value::String(key.owned()), instruction);
        }
        // A namespace names the exporting module's bindings, so it is caught up before it is
        // read: the module may have assigned to an export since the link ran. The shared borrow
        // decides first, so an ordinary object -- which is every object but these -- never takes
        // the exclusive one, and a read that re-enters through a getter cannot collide with it.
        if let Value::Object(object) = target
            && object.borrow().tracks_namespace_cells()
        {
            object.borrow_mut().refresh_namespace();
        }
        // A `DataView` a shrink has left outside its buffer answers neither of its two
        // measurements: the specification checks the view is still in bounds before it reports
        // either, and a zero here would read as a view that is legitimately empty.
        if let Value::DataView(view) = target
            && (key.matches_ascii("byteLength") || key.matches_ascii("byteOffset"))
            && view.out_of_bounds()
        {
            return Err(ExecutionError::DataViewOutOfBounds);
        }
        let stored = self.stored_property_key(target, key);
        // `Object.prototype.__proto__` is an accessor and the tables have no room for one, so
        // the read reaches it here. An own or inherited property of that name shadows it exactly
        // as it would shadow any accessor further up the chain, which is why this runs only when
        // the walk above found nothing.
        if key.matches_ascii("__proto__") && stored.is_none() && is_reflect_target(target) {
            // `[[GetPrototypeOf]]`, which a proxy answers with a handler rather than a stored
            // link -- so reading `p.__proto__` reports the proxy's prototype, not its target's.
            return self.proxy_get_prototype_of(target, instruction);
        }
        // `Function.prototype.caller` and `.arguments` are the same shape of problem: an
        // accessor on a table that has no room for one. Every function inherits the pair from
        // `%Function.prototype%`, both halves are `%ThrowTypeError%`, and so a read of either
        // name through anything callable throws -- unless the program gave that function an own
        // property of the name, which shadows the accessor exactly as it would any other.
        if stored.is_none()
            && key
                .known_ascii()
                .is_some_and(|name| is_restricted_function_property(target, name.as_ref()))
        {
            return Err(restricted_property_error());
        }
        // The same shape once more: an accessor a built-in *prototype* owns is a call, and the
        // tables have no room for one. `Map.prototype.size` is a `TypeError` for exactly this
        // reason -- the prototype is not a Map -- where the table answered `undefined`.
        if stored.is_none()
            && let Some(builtin) = key
                .known_ascii()
                .and_then(|name| machine_accessor(target, name.as_ref()))
        {
            return self.builtin_accessor(builtin, target, instruction);
        }
        // And once more for `Temporal`, whose forty getters live in a table of the same shape.
        // Reading one off the prototype is the getter's own TypeError -- the prototype is not a
        // duration -- where the table answered `undefined`.
        if stored.is_none()
            && let Value::Native(Native::Temporal(temporal::Op::Prototype(kind)), _) = target
            && let Some(field) = key
                .known_ascii()
                .and_then(|name| temporal::prototype_accessor(*kind, name.as_ref()))
        {
            return self.temporal_dispatch(
                temporal::Op::Accessor(*kind, field),
                target,
                &[],
                instruction,
            );
        }
        match stored {
            Some(Property::Accessor { get, .. }) => match get {
                Some(getter) => self.call_value(&getter, target.clone(), Vec::new(), instruction),
                // Setter-only, so there is nothing to read.
                None => Ok(Value::Undefined),
            },
            Some(Property::Value(value)) => Ok(value),
            _ => {
                let found = self.property_key(target, key);
                // Nothing in any store, so a proxy further up the chain is the one thing that
                // could still answer. `OrdinaryGet` keeps the *original* receiver as it walks,
                // which is what the trap's third argument is for.
                if matches!(found, Value::Undefined)
                    && let Some(proxy) = self.proxy_on_chain(target)
                {
                    return self.proxy_get_with_receiver(
                        &proxy,
                        &Value::String(key.owned()),
                        target,
                        instruction,
                    );
                }
                Ok(found)
            }
        }
    }

    /// Route a write through a setter found anywhere on the prototype chain, answering whether
    /// the write was consumed.
    ///
    /// A setter takes the write ahead of creating an own data property, and a getter-only
    /// accessor swallows it. A store that skips this check silently *shadows* the accessor
    /// instead -- which is how `chalk.level = 3` became an ordinary property on the chalk
    /// function and never reached the generator sitting behind the accessor.
    pub(crate) fn inherited_setter_key(
        &mut self,
        store: &RefCell<ObjectData>,
        target: &Value,
        key: PropertyKeyRef<'_>,
        value: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        // Bound out of the `if let`: the borrow would otherwise still be alive at the setter
        // call, which is ordinary code that may touch this very object.
        let existing = store.borrow().lookup_key(key);
        let Some(Property::Accessor { set, .. }) = existing else {
            return Ok(false);
        };
        let Some(setter) = set else {
            // Getter-only: the write is discarded in sloppy mode and a TypeError in strict.
            // Modules are strict, but a script is not, and this is shared.
            return Ok(true);
        };
        self.call_value(&setter, target.clone(), vec![value.clone()], instruction)?;
        Ok(true)
    }

    /// Route a write up the prototype chain a value's *kind* gives it, answering whether the
    /// chain consumed it.
    ///
    /// [`Vm::inherited_setter_key`] walks a store, and an exotic object's chain is not one: it ends
    /// at an intrinsic prototype whose accessors are match arms rather than slots. Every
    /// measurement such a prototype publishes -- a view's `length`, a buffer's `byteLength`, a
    /// regexp's `source` -- is getter-only, so a write to one is swallowed there. Without this
    /// the write would shadow the accessor with an own property, and `sample.length = 9` would
    /// answer 9 for a view of four elements.
    pub(crate) fn chain_setter_key(
        &mut self,
        target: &Value,
        key: PropertyKeyRef<'_>,
        value: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        let mut current = prototype_of(target);
        for _ in 0..MAX_PROTOTYPE_DEPTH {
            if matches!(current, Value::Null | Value::Undefined) {
                return Ok(false);
            }
            match own_property_state_key(&self.intrinsics, &current, key) {
                Some((Property::Accessor { set, .. }, _)) => {
                    // The receiver is the exotic object the program wrote to, not the prototype
                    // the setter was found on.
                    if let Some(setter) = set {
                        self.call_value(&setter, target.clone(), vec![value.clone()], instruction)?;
                    }
                    return Ok(true);
                }
                // A data property up the chain does not take the write -- an own one is created
                // to shadow it -- unless it refused to be written at all.
                Some((Property::Value(_), attributes)) => return Ok(!attributes.writable),
                None => current = prototype_of(&current),
            }
        }
        Ok(false)
    }

    pub(crate) fn property_key(&self, target: &Value, key: PropertyKeyRef<'_>) -> Value {
        if let Some(name) = key.known_ascii() {
            return self.property(target, name.as_ref());
        }
        // Numeric and string-indexed exotics still have an exact answer for an unknown units
        // spelling. A non-ASCII/lone-surrogate name otherwise cannot name one of the scalar
        // intrinsic tables, and its exact own store has already been checked by the caller.
        if matches!(target, Value::Array(_)) && array_index_key(key).is_some() {
            return self.computed_key_ref(target, key);
        }
        if matches!(target, Value::TypedArray(_)) && canonical_numeric_index_key(key).is_some() {
            return self.computed_key_ref(target, key);
        }
        if matches!(target, Value::String(_)) && array_index_key(key).is_some() {
            return self.computed_key_ref(target, key);
        }
        Value::Undefined
    }
}
impl<H: HostOps + ?Sized> Vm<'_, H> {
    pub(crate) fn computed_key_ref(&self, target: &Value, key: PropertyKeyRef<'_>) -> Value {
        if let Value::Array(items) = target
            && let Some(index) = array_index_key(key)
        {
            // A hole *reads* as undefined even though it is not one, which is why the marker
            // never escapes into an expression -- but only after the chain has been asked, since
            // a hole is an absence and `Array.prototype[1] = 1` fills exactly that.
            let element = items.borrow().get(index).cloned();
            return match element {
                Some(value) if !Self::is_hole(&value) => value,
                _ => match array_inherited_index_key(&self.intrinsics, items, key) {
                    Some(Property::Value(value)) => value,
                    // An inherited accessor is a call, which this pure read cannot make. The
                    // computed read above it consults the same chain and does.
                    _ => Value::Undefined,
                },
            };
        }
        if let Value::TypedArray(view) = target
            && let Some(index) = canonical_numeric_index_key(key)
        {
            // Every element key is the view's own answer, and `undefined` when it reaches no
            // slot. Never an ordinary property and never a question for the chain: that is what
            // separates `sample[1.5]`, which is `undefined` however the prototype is decorated,
            // from `sample["1.0"]`, which is an ordinary name and inherits like any other.
            return valid_integer_index(view, index)
                .and_then(|at| view.get(at))
                .unwrap_or(Value::Undefined);
        }
        // `s[0]` is the character at that index, as a one-character string. Indices are UTF-16
        // code units, which is the unit `length` counts in, so the two agree on what is in range.
        if let Value::String(text) = target
            && let Some(index) = array_index_key(key)
        {
            let units: Vec<u16> = text.encode_utf16().collect();
            return match units.get(index) {
                // One unit, not one character: indexing a surrogate pair gives half of it, which
                // is the language's answer and the reason `for-of` over a string differs.
                Some(unit) => Value::String(StringUnits::from_units(&[*unit])),
                None => Value::Undefined,
            };
        }
        self.property_key(target, key)
    }

    /// `delete target[key]`, running a proxy's `deleteProperty` trap when there is one.
    pub(crate) fn delete(
        &mut self,
        target: &Value,
        key: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        let key = &self.computed_key(key, instruction)?;
        let Value::Proxy(proxy) = target else {
            let units = key.property_key_units_checked()?;

            return Ok(delete_property_key(
                &self.intrinsics,
                target,
                PropertyKeyRef::from_value(key, &units),
            ));
        };
        let proxy = Rc::clone(proxy);
        let inner = proxy.target.clone();
        let Some(trap) = self.proxy_trap(&proxy, "deleteProperty", instruction)? else {
            return self.delete(&inner, key, instruction);
        };
        let handler = proxy.handler.clone();
        let arguments = vec![inner.clone(), self.trap_key(key)];
        let answered = self
            .call_value(&trap, handler, arguments, instruction)?
            .is_truthy();
        if !answered {
            return Ok(false);
        }
        // Reporting a removal that did not happen is the lie here, and it is only a lie about a
        // property the target still has and could not have given up.
        match self.own_state(&inner, key, instruction)? {
            Some((_, attributes)) if !attributes.configurable => Err(Self::proxy_invariant(
                "deleteProperty cannot report a non-configurable property of the target removed",
            )),
            Some(_) if !self.target_extensible(&inner, instruction)? => Err(Self::proxy_invariant(
                "deleteProperty cannot report a property of a non-extensible target removed",
            )),
            _ => Ok(true),
        }
    }

    /// The own enumerable string keys of a value, running a proxy's `ownKeys` trap.
    pub(crate) fn own_keys(
        &mut self,
        target: &Value,
        instruction: usize,
    ) -> Result<Vec<StringUnits>, ExecutionError> {
        let Value::Proxy(proxy) = target else {
            // Symbol keys included: `Reflect.ownKeys`, `Object.assign` and `Object.freeze` all
            // reach a proxy's target through here, and all three are defined over both kinds.
            // Internal slots stay out -- copying a collection's `@@entries` would be copying the
            // machine rather than the object.
            return Ok(own_property_keys_with_symbols_units(
                &self.intrinsics,
                target,
            ));
        };
        let proxy = Rc::clone(proxy);
        let inner = proxy.target.clone();
        let Some(trap) = self.proxy_trap(&proxy, "ownKeys", instruction)? else {
            return self.own_keys(&inner, instruction);
        };
        let handler = proxy.handler.clone();
        let listed = self.call_value(&trap, handler, vec![inner.clone()], instruction)?;
        // `CreateListFromArrayLike(trapResult, « String, Symbol »)`: a list of keys, and only
        // keys. The result is any object with a length and indexed properties, not only an Array;
        // reading it through the VM also preserves observable length/index getters. An array
        // hole reads as undefined and is rejected below just like any other non-key.
        if !is_an_object(&listed) {
            return Err(Self::proxy_invariant("ownKeys must report a list of keys"));
        }
        let length = bounded_length(
            self.read_property(&listed, "length", instruction)?
                .as_number(),
        )?;
        let mut keys: Vec<StringUnits> = Vec::new();
        for index in 0..length {
            let entry = self.read_property(&listed, &index.to_string(), instruction)?;
            if !matches!(entry, Value::String(_) | Value::Symbol(_)) {
                return Err(Self::proxy_invariant(
                    "ownKeys must report strings and symbols only",
                ));
            }
            let key = entry.property_key_units_checked()?;
            keys.push(key);
        }
        // Finish CreateListFromArrayLike before checking duplicates: later indexed getters
        // are observable even when an earlier pair repeats. Keep order in `keys`, but use a
        // membership index so reversing a large target's key list does not take quadratic time.
        let mut key_set = HashSet::with_capacity(keys.len());
        for key in &keys {
            if !key_set.insert(key) {
                return Err(Self::proxy_invariant("ownKeys must not repeat a key"));
            }
        }
        // A proxy target has its own `[[OwnPropertyKeys]]` operation. Calling the ordinary
        // store helper on it would lose keys hidden behind an inner proxy, and would make the
        // outer proxy's invariant check accept a list that the target itself could not report.
        let held = if matches!(inner, Value::Proxy(_)) {
            self.own_keys(&inner, instruction)?
        } else {
            own_property_keys_including_symbols_units(&self.intrinsics, &inner)
        };
        // Ordinary object slots are stable while this invariant check runs: after the trap and
        // CreateListFromArrayLike have finished, the remaining operations below cannot call
        // program code for this target. Snapshot their flags once so a large ownKeys result does
        // not linearly rescan the target's property vector for every held key. Keep wrappers,
        // proxies, and every exotic receiver on the operation-based path below.
        let ordinary_configurability = match &inner {
            Value::Object(object) if wrapped_primitive(&inner).is_none() => {
                let data = object.borrow();
                let mut configurability =
                    std::collections::HashMap::with_capacity(data.properties.len());
                for slot in &data.properties {
                    // `own_state` finds the first matching slot, so retain that answer even if
                    // an internal caller ever constructs duplicate keys.
                    configurability
                        .entry(slot.key.clone())
                        .or_insert(slot.attributes.configurable);
                }
                Some(configurability)
            }
            _ => None,
        };
        let extensible = self.target_extensible(&inner, instruction)?;
        for key in &held {
            let locked = if let Some(configurability) = &ordinary_configurability {
                configurability
                    .get(key)
                    .is_some_and(|configurable| !configurable)
            } else {
                self.own_state(&inner, &Value::String(key.clone()), instruction)?
                    .is_some_and(|(_, attributes)| !attributes.configurable)
            };
            // A non-configurable key cannot be taken away, so a list that leaves it out is
            // describing an object the target is not. On a target that cannot grow or shrink
            // either, the same is true of *every* key it has.
            if (locked || !extensible) && !key_set.contains(key) {
                return Err(Self::proxy_invariant(
                    "ownKeys must report every own key the target cannot lose",
                ));
            }
        }
        // Both lists are unique, and every held key was checked above. A size mismatch is
        // therefore exactly an extra reported key; no second membership scan is needed.
        if !extensible && keys.len() != held.len() {
            return Err(Self::proxy_invariant(
                "ownKeys cannot report a key a non-extensible target does not have",
            ));
        }
        Ok(keys)
    }

    /// The first proxy on `target`'s prototype chain, not counting `target` itself.
    ///
    /// A proxy is usually reached directly, and then its traps run. Reached *through* an object
    /// that inherits from it, they did not: the walk that finds an inherited property reads
    /// property stores, and a proxy has none for it to read, so the chain simply ended there.
    /// `Object.create(new Proxy({}, { get, has }))` answered `undefined` for every property and
    /// `false` for every `in` -- the traps never ran, and a proxy used as a prototype is how a
    /// library gives an object dynamic members it cannot enumerate in advance.
    pub(crate) fn proxy_on_chain(&self, target: &Value) -> Option<Value> {
        let mut current = prototype_of(target);
        for _ in 0..MAX_PROTOTYPE_DEPTH {
            match current {
                Value::Proxy(_) => return Some(current),
                Value::Null | Value::Undefined => return None,
                other => current = prototype_of(&other),
            }
        }
        None
    }

    /// Does `target` have `key`, running a proxy's `has` trap.
    pub(crate) fn has(
        &mut self,
        target: &Value,
        key: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        let Value::Proxy(proxy) = target else {
            if has_property_checked(&self.intrinsics, target, key)? {
                return Ok(true);
            }
            // Nothing found by walking the stores, so a proxy further up is the one thing that
            // could still answer yes.
            if let Some(proxy) = self.proxy_on_chain(target) {
                return self.has(&proxy, key, instruction);
            }
            return Ok(false);
        };
        let proxy = Rc::clone(proxy);
        let inner = proxy.target.clone();
        let Some(trap) = self.proxy_trap(&proxy, "has", instruction)? else {
            return self.has(&inner, key, instruction);
        };
        let handler = proxy.handler.clone();
        let arguments = vec![inner.clone(), self.trap_key(key)];
        let answered = self
            .call_value(&trap, handler, arguments, instruction)?
            .is_truthy();
        if answered {
            return Ok(true);
        }
        // Denying a property is a lie only when the target could not have lost it.
        match self.own_state(&inner, key, instruction)? {
            Some((_, attributes)) if !attributes.configurable => Err(Self::proxy_invariant(
                "has cannot deny a non-configurable property of the target",
            )),
            Some(_) if !self.target_extensible(&inner, instruction)? => Err(Self::proxy_invariant(
                "has cannot deny a property of a non-extensible target",
            )),
            _ => Ok(false),
        }
    }

    /// A computed read that can run a proxy's `get` trap.
    ///
    /// `computed` itself is pure and cannot call anything, which is fine for every other kind of
    /// value and not fine for a proxy: reading one *is* a call.
    pub(crate) fn read_computed(
        &mut self,
        target: &Value,
        key: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        self.read_computed_with_receiver(target, key, target, instruction)
    }

    /// The `[[Get]]` operation with an explicit receiver. Ordinary bracket access uses the target
    /// itself; `Reflect.get` can provide a different value for accessors and proxy traps.
    /// The computed spelling of [`Vm::read_property`], with the same dead-zone refusal.
    pub(crate) fn read_computed_with_receiver(
        &mut self,
        target: &Value,
        key: &Value,
        receiver: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let value = self.read_computed_with_receiver_inner(target, key, receiver, instruction)?;
        if matches!(value, Value::Native(Native::Uninitialized, _)) {
            return Err(uninitialized_export_error(&key.to_display_string()));
        }
        Ok(value)
    }

    pub(crate) fn read_computed_with_receiver_inner(
        &mut self,
        target: &Value,
        key: &Value,
        receiver: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let key = &self.computed_key(key, instruction)?;
        if matches!(target, Value::Proxy(_)) {
            return self.proxy_get_with_receiver(target, key, receiver, instruction);
        }
        // A numeric key on an integer-indexed exotic already identifies the canonical
        // numeric string. Avoid formatting and reparsing it for every byte of a view copy.
        // ToPropertyKey maps numeric -0 to "0"; the distinct string "-0" still takes the
        // generic path. Invalid numeric indices never consult the prototype or receiver.
        if let (Value::TypedArray(view), Value::Number(number)) = (target, key) {
            let index = if *number == 0.0 { 0.0 } else { *number };
            return Ok(valid_integer_index(view, index)
                .and_then(|at| view.get(at))
                .unwrap_or(Value::Undefined));
        }
        let property_key_units = key.property_key_units_checked()?;

        let property_key = PropertyKeyRef::from_value(key, &property_key_units);

        // The same accessors the named read consults, for the computed spelling: a symbol key
        // reaches `Symbol.species` only through here.
        if let Some(builtin) = property_key
            .known_ascii()
            .and_then(|name| machine_accessor(target, name.as_ref()))
            && self.stored_property_key(target, property_key).is_none()
        {
            return self.builtin_accessor(builtin, receiver, instruction);
        }
        // The named read's own walk, on the computed spelling. This was a second copy of it,
        // arm for arm, with two of the arms already delegating here -- and the copy is how the
        // two spellings came to disagree: `Object.defineProperty(sample, key, { get })` with a
        // variable `key` was answered by the prototype the getter was shadowing. One walk cannot
        // drift from itself.
        match self.stored_property_key(target, property_key) {
            Some(Property::Accessor { get, .. }) => match get {
                Some(getter) => self.call_value(&getter, receiver.clone(), Vec::new(), instruction),
                None => Ok(Value::Undefined),
            },
            Some(Property::Value(value)) => Ok(value),
            _ => Ok(self.computed_key_ref(target, property_key)),
        }
    }

    /// Read `key` from a proxy, through its `get` trap or through its target.
    pub(crate) fn proxy_get(
        &mut self,
        target: &Value,
        key: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        self.proxy_get_with_receiver(target, key, target, instruction)
    }

    pub(crate) fn proxy_get_with_receiver(
        &mut self,
        target: &Value,
        key: &Value,
        receiver: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Proxy(proxy) = target else {
            return self.read_computed_with_receiver(target, key, receiver, instruction);
        };
        let proxy = Rc::clone(proxy);
        let inner = proxy.target.clone();
        let Some(trap) = self.proxy_trap(&proxy, "get", instruction)? else {
            return self.read_computed_with_receiver(&inner, key, receiver, instruction);
        };
        let handler = proxy.handler.clone();
        let arguments = vec![inner.clone(), self.trap_key(key), receiver.clone()];
        let answered = self.call_value(&trap, handler, arguments, instruction)?;
        // A property the target has promised never to change is the one thing a `get` trap may
        // not invent an answer for: a caller that read it once may hold the value forever.
        match self.own_state(&inner, key, instruction)? {
            Some((Property::Value(held), attributes))
                if !attributes.configurable && !attributes.writable =>
            {
                if !same_value(&answered, &held) {
                    return Err(Self::proxy_invariant(
                        "get must report the target's value for a non-writable, \
                         non-configurable property",
                    ));
                }
            }
            Some((Property::Accessor { get: None, .. }, attributes))
                if !attributes.configurable && !matches!(answered, Value::Undefined) =>
            {
                return Err(Self::proxy_invariant(
                    "get must report undefined for a non-configurable accessor with no getter",
                ));
            }
            _ => {}
        }
        Ok(answered)
    }

    /// `PutValue` for an assignment the program wrote: `Set(O, P, V, strict)`.
    ///
    /// The whole of what strictness changes about a write. Sloppy code discards a refused
    /// assignment -- to a frozen object, a non-writable property, a getter with no setter, a
    /// property of a primitive -- and strict code raises a `TypeError` for the same refusal.
    /// The two are one instruction, so the running frame is what says which.
    pub(crate) fn assign(
        &mut self,
        target: &Value,
        key: &Value,
        value: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        self.assign_with_strict(target, key, value, instruction, self.strict)
    }

    pub(crate) fn assign_with_strict(
        &mut self,
        target: &Value,
        key: &Value,
        value: Value,
        instruction: usize,
        strict: bool,
    ) -> Result<(), ExecutionError> {
        // IntegerIndexedElementSet accepts invalid indices in both strict and sloppy code,
        // after converting the value. Only a genuine numeric key and view take this path;
        // proxies and object keys retain their traps and ToPropertyKey ordering.
        if let (Value::TypedArray(view), Value::Number(number)) = (target, key) {
            let index = if *number == 0.0 { 0.0 } else { *number };
            return self.set_typed_array_index(view, index, &value, instruction);
        }
        if !strict {
            return self.set_computed(target, key, value, instruction);
        }
        // `PutValue` performs `ToPropertyKey` once, before `[[Set]]` decides whether the
        // write is accepted. Keep the converted value for both the descriptor probe and the
        // write so a key object's `toString` cannot run twice on a strict assignment.
        let computed_key = self.computed_key(key, instruction)?;
        // A primitive has no property store, so an ordinary assignment onto one creates
        // nothing -- which sloppy code cannot tell from a write and strict code must.
        // An inherited setter is the exception: it is a call, and the primitive is its receiver.
        if matches!(
            target,
            Value::String(_) | Value::Number(_) | Value::Boolean(_) | Value::BigInt(_)
        ) {
            let units = computed_key.property_key_units_checked()?;
            let setter = match own_property_state_key(
                &self.intrinsics,
                &prototype_of(target),
                PropertyKeyRef::Units(&units),
            ) {
                Some((Property::Accessor { set, .. }, _)) => set,
                _ => None,
            };
            return match setter {
                Some(setter) => self
                    .call_value(&setter, target.clone(), vec![value], instruction)
                    .map(|_| ()),
                None => Err(Self::write_refused(&computed_key)),
            };
        }
        self.set_or_throw(target, &computed_key, value, instruction)
    }

    /// `DeletePropertyOrThrow` where the program wrote `delete` in strict code, and the plain
    /// `[[Delete]]` where it did not: a refused delete is `false` in sloppy mode and a
    /// `TypeError` in strict.
    pub(crate) fn remove(
        &mut self,
        target: &Value,
        key: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        self.remove_with_strict(target, key, instruction, self.strict)
    }

    pub(crate) fn remove_with_strict(
        &mut self,
        target: &Value,
        key: &Value,
        instruction: usize,
        strict: bool,
    ) -> Result<bool, ExecutionError> {
        // `DeletePropertyOrThrow` converts its key once. The plain delete below receives the
        // primitive result so a side-effecting key object is not coerced a second time while
        // formatting a strict-mode refusal.
        let computed_key = self.computed_key(key, instruction)?;
        let removed = self.delete(target, &computed_key, instruction)?;
        if !removed && strict {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!(
                    "Cannot delete property '{}' of an object",
                    computed_key.property_key_lossy()
                ),
            });
        }
        Ok(removed)
    }

    /// Whether a missing array element can be created. An inherited setter is consulted
    /// before this storage constraint because calling it creates no own element.
    pub(crate) fn array_index_creation_allowed(items: &ArrayData, index: usize) -> bool {
        items.properties.borrow().is_extensible()
            && (index < items.borrow().len() || items.length_writable.get())
    }

    /// The indexed array write shared by numeric and string keys. Descriptor/refusal handling
    /// stays here so avoiding a numeric key's string allocation cannot bypass an exotic element.
    pub(crate) fn set_array_index(
        &mut self,
        target: &Value,
        items: &Rc<ArrayData>,
        index: usize,
        value: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        // An element the program defined answers for itself: a setter is a call, and
        // a data property that is not writable discards the write the same way a
        // frozen object's does.
        let exotic = array_exotic_element(items, index);
        let has_own_element = exotic.is_some()
            || items
                .borrow()
                .get(index)
                .is_some_and(|value| !Self::is_hole(value));
        if let Some(entry) = exotic {
            match entry.property {
                Property::Accessor { set, .. } => {
                    if let Some(setter) = set {
                        self.call_value(&setter, target.clone(), vec![value], instruction)?;
                    }
                    return Ok(());
                }
                Property::Value(_) if !entry.attributes.writable => return Ok(()),
                Property::Value(_) => {}
            }
        }
        if !has_own_element {
            // Ordinary arrays with untouched intrinsic numeric properties cannot encounter
            // a setter. Otherwise reuse the same descriptor walk as the other exotic stores.
            if (self.intrinsics.indexed_array_prototype.get()
                || items.properties.borrow().prototype().is_some())
                && self.chain_setter_key(
                    target,
                    PropertyKeyRef::Utf8(&index.to_string()),
                    &value,
                    instruction,
                )?
            {
                return Ok(());
            }
            if !Self::array_index_creation_allowed(items, index) {
                return Ok(());
            }
        }
        if items.properties.borrow().is_frozen() {
            return Ok(());
        }
        let mut elements = items.borrow_mut();
        // Writing past the end extends with *holes*, not undefined: `a[3] = 4` on a
        // one-element array leaves two positions that `in` denies, `for...in` skips
        // and a console shows as `<2 empty items>`. A sparse array would pay nothing
        // for a distant index; this one pays a slot each, so the same bound the
        // constructor uses applies here.
        if index >= elements.len() {
            if index >= MAX_DENSE_ARRAY {
                return Err(ExecutionError::ArrayTooLarge { length: index + 1 });
            }
            items.sparse.set(true);
            elements.resize(index + 1, Value::Native(Native::Hole, RealmId::PRIMARY));
        }
        elements[index] = value;
        Ok(())
    }

    /// The integer-indexed write, shared by numeric and canonical string keys. Coercion
    /// must precede the bounds check because it can detach or resize the buffer.
    fn set_typed_array_index(
        &mut self,
        view: &Rc<TypedArrayData>,
        index: f64,
        value: &Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let element = self.as_element(view.kind, value, instruction)?;
        if let Some(at) = valid_integer_index(view, index) {
            view.set(at, &element)?;
        }
        Ok(())
    }

    pub(crate) fn set_computed(
        &mut self,
        target: &Value,
        key: &Value,
        value: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let key = &self.computed_key(key, instruction)?;
        if let Value::Proxy(proxy) = target {
            let proxy = Rc::clone(proxy);
            let inner = proxy.target.clone();
            return match self.proxy_trap(&proxy, "set", instruction)? {
                Some(trap) => {
                    let handler = proxy.handler.clone();
                    let arguments = vec![
                        inner.clone(),
                        self.trap_key(key),
                        value.clone(),
                        target.clone(),
                    ];
                    // Sloppy code discards a refusal, so the answer is not consulted for a
                    // throw -- but a *truthy* answer still has to be true, and that is a
                    // different question the invariant asks whatever the strictness.
                    let answered = self.call_value(&trap, handler, arguments, instruction)?;
                    if !answered.is_truthy() {
                        return Ok(());
                    }
                    self.check_set_invariant(&inner, key, &value, instruction)
                }
                None => self.set_computed(&inner, key, value, instruction),
            };
        }
        // Numeric array indices are already primitive property keys. Keep their integer
        // representation instead of allocating a decimal string and UTF-16 copy to parse back.
        // -0 names element zero, but 2**32-1 and non-integers remain ordinary property names.
        if let (Value::Array(items), Value::Number(number)) = (target, key)
            && *number >= 0.0
            && *number < f64::from(u32::MAX)
            && number.fract() == 0.0
        {
            return self.set_array_index(target, items, *number as usize, value, instruction);
        }
        if let (Value::TypedArray(view), Value::Number(number)) = (target, key) {
            let index = if *number == 0.0 { 0.0 } else { *number };
            return self.set_typed_array_index(view, index, &value, instruction);
        }
        let key_units = key.property_key_units_checked()?;

        let property_key = PropertyKeyRef::from_value(key, &key_units);

        if let Value::TypedArray(view) = target
            && let Some(index) = canonical_numeric_index_key(property_key)
        {
            return self.set_typed_array_index(view, index, &value, instruction);
        }
        // The other half of `Object.prototype.__proto__`. A value that is neither an object nor
        // null is *ignored* rather than refused -- the setter has no error to raise for one --
        // and either way nothing is stored under the name.
        if property_key.matches_ascii("__proto__") && is_reflect_target(target) {
            match (&value, target) {
                (Value::Object(_) | Value::Function(_) | Value::Array(_) | Value::Null, _) => {}
                _ => return Ok(()),
            }
            // `o.__proto__ = p` runs the setter, and the setter refuses a cycle or a
            // non-extensible target. Storing it anyway is how `a.__proto__ = b` on an object `b`
            // already inherits from produced a chain whose walk never ends.
            if !prototype_may_change(&self.intrinsics, target, &value) {
                return Err(prototype_change_refused());
            }
            // A value with no store keeps its chain in its kind and has nowhere to put another.
            record_prototype(target, value);
            return Ok(());
        }
        // The other half of the restricted pair. Its setter is `%ThrowTypeError%` too, so a
        // write throws rather than shadowing the accessor with an own property -- which is what
        // `f.caller = {}` does in every engine, in sloppy mode as well as strict.
        if property_key
            .known_ascii()
            .is_some_and(|name| is_restricted_function_property(target, name.as_ref()))
            && self.stored_property_key(target, property_key).is_none()
        {
            return Err(restricted_property_error());
        }
        match target {
            Value::Shared(_) => Ok(()),
            // Indexed writes still reach their descriptor walk: even a frozen array may
            // inherit a setter, and invoking it does not create or change an own element.
            Value::Array(items)
                if items.properties.borrow().is_frozen()
                    && array_index_key(property_key).is_none() =>
            {
                Ok(())
            }
            Value::Array(items) => match array_index_key(property_key) {
                Some(index) => self.set_array_index(target, items, index, value, instruction),
                // `length` is derived, not stored: writing it truncates or extends. The write
                // runs the same `ArraySetLength` a definition does, so `arr.length = 0` and
                // `Object.defineProperty(arr, "length", {value: 0})` cannot disagree about an
                // element that will not go. A refusal is discarded rather than raised, for the
                // same reason a frozen object's write is: the machine does not know whether the
                // code that wrote it was strict.
                None if property_key.matches_ascii("length") => {
                    // `ToUint32` of whatever the program supplied, and for an object that
                    // conversion *calls* its `valueOf` or `toString`. The pure `as_number`
                    // inside `array_set_length` answered NaN for one, which the range check
                    // then reported as `Invalid array length` -- a refusal where the language
                    // takes the value.
                    let items = Rc::clone(items);
                    let value = Value::Number(self.as_number_argument(Some(&value), instruction)?);
                    array_set_length(
                        &items,
                        &Descriptor {
                            value: Some(value),
                            get: None,
                            set: None,
                            writable: None,
                            enumerable: None,
                            configurable: None,
                        },
                    )?;
                    Ok(())
                }
                // Not an index, so it is an ordinary named property of the array.
                None => {
                    if self.inherited_setter_key(
                        &items.properties,
                        target,
                        property_key,
                        &value,
                        instruction,
                    )? {
                        return Ok(());
                    }
                    items.properties.borrow_mut().set_key(property_key, value);
                    Ok(())
                }
            },
            // Every other name on an exotic object is an ordinary property of an ordinary
            // object, so the write walks the chain exactly as it would on one: an own accessor
            // takes it, the chain's accessor takes it, a getter-only accessor swallows it, and
            // anything left becomes an own property of the store beside the value. Discarding it
            // outright made `sample.constructor = C` look like it worked and read back as the
            // view's own kind.
            other if exotic_properties(other).is_some() => {
                let Some(store) = exotic_properties(target) else {
                    return Ok(());
                };
                if self.inherited_setter_key(store, target, property_key, &value, instruction)? {
                    return Ok(());
                }
                if self.chain_setter_key(target, property_key, &value, instruction)? {
                    return Ok(());
                }
                store.borrow_mut().set_key(property_key, value);
                Ok(())
            }
            Value::Object(object) => {
                if property_key.matches_ascii("statusCode")
                    && let Some(NodeObject::HttpResponse(state)) = &object.borrow().node_object
                {
                    let status = value.as_number();
                    if !(100.0..=999.0).contains(&status) || status.fract() != 0.0 {
                        return Err(ExecutionError::InvalidHttpStatus);
                    }
                    state.borrow_mut().status = status as u16;
                    return Ok(());
                }
                if self.inherited_setter_key(object, target, property_key, &value, instruction)? {
                    return Ok(());
                }
                // No own property and no inherited setter, so a proxy further up the chain is
                // what `OrdinarySetWithOwnDescriptor` reaches next -- its `set` trap decides,
                // and creating an own property here instead would shadow it silently.
                if object.borrow().lookup_key(property_key).is_none()
                    && let Some(proxy) = self.proxy_on_chain(target)
                {
                    self.set_computed(&proxy, key, value, instruction)?;
                    return Ok(());
                }
                object.borrow_mut().set_key(property_key, value);
                Ok(())
            }
            Value::Function(closure) => {
                // `f.prototype = {...}` replaces the object `new f()` will link instances to,
                // which is a slot on the closure rather than an ordinary property -- so a write
                // that landed in the property store would be read back by `f.prototype` and
                // ignored by `new`.
                if property_key.matches_ascii("prototype") {
                    if closure.properties.borrow().is_frozen() {
                        return Ok(());
                    }
                    *closure.prototype.borrow_mut() = Some(value);
                    return Ok(());
                }
                if self.inherited_setter_key(
                    &closure.properties,
                    target,
                    property_key,
                    &value,
                    instruction,
                )? {
                    return Ok(());
                }
                closure.properties.borrow_mut().set_key(property_key, value);
                Ok(())
            }
            Value::Bound(bound) => {
                if self.inherited_setter_key(
                    &bound.properties,
                    target,
                    property_key,
                    &value,
                    instruction,
                )? {
                    return Ok(());
                }
                bound.properties.borrow_mut().set_key(property_key, value);
                Ok(())
            }
            // A built-in is an object, and the specification makes almost every own property of
            // one writable: `Math.max = f` and `Array.prototype.mine = f` are ordinary
            // assignments. Refusing them as "not an object" was this engine reporting that it
            // had nowhere to put the answer, not that the language forbade the write.
            Value::Native(_, _) | Value::Callback(_, _) => {
                if matches!(
                    target,
                    Value::Native(Native::PrototypeOf(PrototypeTable::Array), _)
                ) && property_key.matches_ascii("length")
                {
                    let number = self.as_number_argument(Some(&value), instruction)?;
                    if !number.is_finite()
                        || number < 0.0
                        || number.fract() != 0.0
                        || number > MAX_ARRAY_LENGTH
                    {
                        return Err(ExecutionError::InvalidArrayLength);
                    }
                    let attributes = own_attributes_key(&self.intrinsics, target, property_key);
                    if attributes.is_some_and(|attributes| !attributes.writable) {
                        return Ok(());
                    }
                    self.intrinsics.install_key(
                        target,
                        property_key,
                        Property::Value(Value::Number(number)),
                        attributes.unwrap_or(Attributes {
                            writable: true,
                            enumerable: false,
                            configurable: false,
                        }),
                    );
                    return Ok(());
                }
                // Bound out of the `if let`, because the setter is ordinary code that may write
                // to this very built-in.
                let existing = own_property_state_key(&self.intrinsics, target, property_key);
                if let Some((Property::Accessor { set, .. }, _)) = existing {
                    // Getter-only swallows the write, exactly as it does on an ordinary object.
                    if let Some(setter) = set {
                        self.call_value(&setter, target.clone(), vec![value], instruction)?;
                    }
                    return Ok(());
                }
                let attributes = own_attributes_key(&self.intrinsics, target, property_key);
                // A property that promised not to change does not. Sloppy mode discards the
                // write and strict mode throws; the machine does not know which it is in, and
                // discarding is the one that cannot invent an error.
                if attributes.is_some_and(|attributes| !attributes.writable) {
                    return Ok(());
                }
                // An assignment keeps the flags the property already had and gives a new one the
                // ordinary three, which is what it does on every other kind of object.
                self.intrinsics.install_key(
                    target,
                    property_key,
                    Property::Value(value),
                    attributes.unwrap_or(Attributes::DEFAULT),
                );
                if matches!(
                    target,
                    Value::Native(Native::PrototypeOf(PrototypeTable::Array), _)
                ) && let Some(index) = array_index_key(property_key)
                {
                    let cur_len = match own_property_state_key(
                        &self.intrinsics,
                        target,
                        PropertyKeyRef::Utf8("length"),
                    ) {
                        Some((Property::Value(v), _)) => v.as_number(),
                        _ => 0.0,
                    };
                    if (index as f64) >= cur_len {
                        self.intrinsics.install_key(
                            target,
                            PropertyKeyRef::Utf8("length"),
                            Property::Value(Value::Number((index + 1) as f64)),
                            Attributes {
                                writable: true,
                                enumerable: false,
                                configurable: false,
                            },
                        );
                    }
                }
                Ok(())
            }
            other => Err(ExecutionError::NotAnObject {
                instruction,
                value: other.type_name(),
            }),
        }
    }
}
impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// Allocate a `Symbol(description)` and remember it by serial.
    ///
    /// The remembering is the point. A symbol-keyed property is stored under the *string*
    /// `@@symbol:{serial}`, so the object holds the key and not the symbol -- and
    /// `Object.getOwnPropertySymbols` has to answer with the symbol itself, the same one by
    /// identity. Nothing else can produce it: `Symbol()` deliberately makes a different symbol
    /// every time it is called.
    ///
    /// Serials come from the module realm when there is one, so that two modules of the same
    /// program cannot mint the same key for two different symbols.
    pub(crate) fn new_symbol(&mut self, description: Option<StringUnits>) -> Value {
        let allocate = |serial: &mut u64, symbols: &mut Vec<Rc<UniqueSymbol>>| {
            *serial += 1;
            let symbol = Rc::new(UniqueSymbol {
                serial: *serial,
                description,
            });
            symbols.push(Rc::clone(&symbol));
            Value::Symbol(Symbol::Unique(symbol))
        };
        match &self.module_realm {
            Some(realm) => {
                let mut realm = realm.borrow_mut();
                let ModuleRealmShared {
                    symbol_serial,
                    symbols,
                    ..
                } = &mut *realm;
                allocate(symbol_serial, symbols)
            }
            None => allocate(&mut self.symbol_serial, &mut self.symbols),
        }
    }

    /// The symbol a reserved store key names, or `None` when the key is an internal slot.
    ///
    /// Both halves of the question are answered here because both are the same question: this
    /// interpreter stores a symbol key as a string with a reserved prefix, and so does its own
    /// bookkeeping -- `@@entries` for a collection, `@@trace` on an error, `@@disposeEntries` on
    /// a stack. Reflection must report the first kind and must never report the second, and this
    /// is the one place that line is drawn.
    pub(crate) fn symbol_of_key(&self, key: &str) -> Option<Symbol> {
        if let Some(symbol) = WellKnown::from_property_key(key) {
            return Some(Symbol::WellKnown(symbol));
        }
        let serial = Symbol::serial_in_key(key)?;
        let held = |symbols: &[Rc<UniqueSymbol>]| {
            symbols
                .iter()
                .find(|symbol| symbol.serial == serial)
                .map(|symbol| Symbol::Unique(Rc::clone(symbol)))
        };
        match &self.module_realm {
            Some(realm) => held(&realm.borrow().symbols),
            None => held(&self.symbols),
        }
    }

    /// The own keys of `target` that a reflection operation reporting symbols sees.
    ///
    /// String keys first in the store's order, then symbol keys in theirs, which is
    /// `OrdinaryOwnPropertyKeys`. Internal slots are dropped: they share the reserved prefix
    /// with symbol keys and are not properties of the object at all.
    pub(crate) fn own_keys_reflected(
        &mut self,
        target: &Value,
        instruction: usize,
    ) -> Result<Vec<Value>, ExecutionError> {
        if let Value::Shared(facade) = target {
            return Ok(facade.own_keys().into_iter().map(Value::String).collect());
        }
        let names = match target {
            // A proxy answers with a trap rather than a store, and the trap already reports
            // both kinds in the trap result's order.
            Value::Proxy(_) => {
                let raw_keys = self.own_keys(target, instruction)?;
                return Ok(raw_keys
                    .into_iter()
                    .map(|key| {
                        if is_reserved_units(&key) {
                            if let Some(name) = PropertyKeyRef::Units(&key).known_ascii() {
                                if let Some(sym) = self.symbol_of_key(name.as_ref()) {
                                    return Value::Symbol(sym);
                                }
                            }
                        }
                        Value::String(key.into())
                    })
                    .collect());
            }
            _ => own_property_keys_units(&self.intrinsics, target),
        };
        let mut reflected: Vec<Value> = names
            .into_iter()
            .map(|value| Value::String(value.into()))
            .collect();
        reflected.extend(
            own_symbol_keys_units(&self.intrinsics, target)
                .into_iter()
                .filter_map(|key| {
                    PropertyKeyRef::Units(&key)
                        .known_ascii()
                        .and_then(|name| self.symbol_of_key(name.as_ref()))
                })
                .map(Value::Symbol),
        );
        Ok(reflected)
    }

    /// The abort algorithm: settle the signal once, then tell everyone who asked.
    ///
    /// Idempotent, which is what makes `controller.abort()` safe to call from a `finally` and
    /// from a timeout that has already fired. The listeners run *after* `aborted` and `reason`
    /// are visible, because that is the state every one of them reads first.
    pub(crate) fn run_abort(
        &mut self,
        signal: &Value,
        reason: Option<Value>,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let Value::Object(object) = signal else {
            return Ok(());
        };
        if object
            .borrow()
            .get_own("aborted")
            .is_some_and(Value::is_truthy)
        {
            return Ok(());
        }
        // The default reason is an `AbortError`, which is what a program tests for when it wants
        // to tell "the caller cancelled this" from "this failed".
        let reason = match reason {
            Some(reason) if !matches!(reason, Value::Undefined) => reason,
            _ => {
                let error = self.construct_error(
                    ErrorKind::Error,
                    &[Value::String(
                        "This operation was aborted".to_owned().into(),
                    )],
                );
                if let Value::Object(error) = &error {
                    error
                        .borrow_mut()
                        .set_non_enumerable("name", Value::String("AbortError".to_owned().into()));
                }
                error
            }
        };
        let listeners = {
            let mut held = object.borrow_mut();
            held.set_non_enumerable("aborted", Value::Boolean(true));
            held.set_non_enumerable("reason", reason);
            let listeners = match held.get_own(ABORT_LISTENERS) {
                Some(Value::Array(listeners)) => listeners.borrow().clone(),
                _ => Vec::new(),
            };
            let handler = held.get_own("onabort").cloned();
            handler
                .filter(is_callable)
                .into_iter()
                .chain(listeners)
                .collect::<Vec<_>>()
        };
        // One event object, shared by every listener, as a real dispatch would be.
        let event = ordinary_object([
            ("type".to_owned(), Value::String("abort".to_owned().into())),
            ("target".to_owned(), signal.clone()),
        ]);
        for listener in listeners {
            self.call_value(&listener, signal.clone(), vec![event.clone()], instruction)?;
        }
        Ok(())
    }

    /// `Reflect.set` returns the boolean result of `[[Set]]` instead of applying the throwing
    /// assignment spelling. The write itself still goes through `set_computed` after the same
    /// descriptor probe used by `set_or_throw`, so setters and proxy traps run only once.
    pub(crate) fn set_reflect(
        &mut self,
        target: &Value,
        key: &Value,
        value: Value,
        receiver: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        if let Value::Proxy(proxy) = target {
            let proxy = Rc::clone(proxy);
            let inner = proxy.target.clone();
            return match self.proxy_trap(&proxy, "set", instruction)? {
                Some(trap) => {
                    let handler = proxy.handler.clone();
                    let arguments = vec![
                        inner.clone(),
                        self.trap_key(key),
                        value.clone(),
                        receiver.clone(),
                    ];
                    let answered = self.call_value(&trap, handler, arguments, instruction)?;
                    if !answered.is_truthy() {
                        // Reported rather than thrown: answering whether the write happened is
                        // what this spelling of the operation exists for.
                        return Ok(false);
                    }
                    self.check_set_invariant(&inner, key, &value, instruction)?;
                    Ok(true)
                }
                None => self.set_reflect(&inner, key, value, receiver, instruction),
            };
        }
        let units = key.property_key_units_checked()?;
        let property_key = PropertyKeyRef::from_value(key, &units);
        // An integer-indexed exotic object answers `[[Set]]` for every *canonical numeric* name
        // itself, whether or not the view has such an element, so the prototype chain is never
        // consulted for one: a setter installed on `%TypedArray%.prototype["0"]` stays
        // unreachable, and `ta["1.5"] = v` is discarded rather than stored under that name.
        if let Value::TypedArray(view) = target
            && let Some(index) = canonical_numeric_index_key(property_key)
        {
            let view = Rc::clone(view);
            if same_value(target, receiver) {
                // `TypedArraySetElement`: the conversion runs first and is observable even when
                // the slot it was aimed at is out of bounds, or was detached by the conversion.
                let element = self.as_element(view.kind, &value, instruction)?;
                if let Some(at) = valid_integer_index(&view, index) {
                    view.set(at, &element)?;
                }
                return Ok(true);
            }
            // A different receiver and an index the view does not have: accepted without even
            // coercing the value, because there is no slot to convert it for.
            if valid_integer_index(&view, index).is_none() {
                return Ok(true);
            }
        }
        self.ordinary_set_reflect_key(target, property_key, value, receiver, instruction)
    }

    /// The ordinary half of `Reflect.set(target, key, value, receiver)`.  The receiver is where
    /// a writable data property is created or updated; it is not silently replaced by target.
    pub(crate) fn ordinary_set_reflect_key(
        &mut self,
        target: &Value,
        key: PropertyKeyRef<'_>,
        value: Value,
        receiver: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        if refuses_every_write(target) {
            return Ok(false);
        }
        let own = own_property_state_key(&self.intrinsics, target, key);
        if let Some((property, attributes)) = own {
            match property {
                Property::Accessor {
                    set: Some(setter), ..
                } => {
                    self.call_value(&setter, receiver.clone(), vec![value], instruction)?;
                    return Ok(true);
                }
                Property::Accessor { set: None, .. } => return Ok(false),
                Property::Value(_) if !attributes.writable => return Ok(false),
                Property::Value(_) => {}
            }
            if !is_reflect_target(receiver) {
                return Ok(false);
            }
            return self.define_on_receiver_key(receiver, key, value, instruction);
        }
        let parent = self.prototype_for(target, instruction)?;
        if !matches!(parent, Value::Null | Value::Undefined) {
            return self.ordinary_set_reflect_key(&parent, key, value, receiver, instruction);
        }
        if !is_reflect_target(receiver) {
            return Ok(false);
        }
        self.define_on_receiver_key(receiver, key, value, instruction)
    }

    /// The tail of `OrdinarySetWithOwnDescriptor`: the write lands on the *receiver*, and what
    /// it is allowed to change there is decided by what the receiver already holds.
    ///
    /// The distinction this makes is the whole of the operation. A receiver that already has an
    /// own data property is redefined with a descriptor carrying **only** `[[Value]]`, so its
    /// writable, enumerable and configurable flags are left exactly as they were; a receiver
    /// that has no own property gets `CreateDataProperty`, which is the full descriptor with all
    /// three flags true. Completing the partial descriptor instead -- filling the three absent
    /// fields with their `false` defaults -- turns the first write through a proxy into a
    /// freeze, and every write after it is refused. That is one write per property, which is
    /// what a reactive store looks like when it stops working.
    pub(crate) fn define_on_receiver_key(
        &mut self,
        receiver: &Value,
        key: PropertyKeyRef<'_>,
        value: Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        let held = self.own_state(receiver, &Value::String(key.owned()), instruction)?;
        let descriptor = match held {
            // An accessor on the receiver is refused outright, whichever half it has: the
            // target's data property is what decided this is a data write, and the receiver
            // never agreed to that.
            Some((Property::Accessor { .. }, _)) => return Ok(false),
            Some((Property::Value(_), attributes)) if !attributes.writable => return Ok(false),
            Some((Property::Value(_), _)) => Descriptor {
                value: Some(value),
                ..Descriptor::default()
            },
            None => Descriptor {
                value: Some(value),
                writable: Some(true),
                enumerable: Some(true),
                configurable: Some(true),
                ..Descriptor::default()
            },
        };
        self.define_own_operation_key(receiver, key, &descriptor, instruction)
    }

    /// `target.[[DefineOwnProperty]](name, descriptor)`, asked as an operation rather than of a
    /// store, so a proxy answers with its trap and keeps the descriptor *partial* while it does.
    pub(crate) fn define_own_operation_key(
        &mut self,
        target: &Value,
        key: PropertyKeyRef<'_>,
        descriptor: &Descriptor,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        let Value::Proxy(proxy) = target else {
            return self.define_own_key(target, key, descriptor, instruction);
        };
        let proxy = Rc::clone(proxy);
        let inner = proxy.target.clone();
        let key = Value::String(key.owned());
        let Some(trap) = self.proxy_trap(&proxy, "defineProperty", instruction)? else {
            // No trap: the definition forwards to the target as the record it is. Round-tripping
            // it through a descriptor object here is what used to complete it.
            let key_units = key.property_key_units_checked()?;
            return self.define_own_operation_key(
                &inner,
                PropertyKeyRef::Units(&key_units),
                descriptor,
                instruction,
            );
        };
        // `FromPropertyDescriptor`, which spells out only the fields the record actually has --
        // a handler that reads `"writable" in descriptor` can see the difference.
        let object = Self::descriptor_object(descriptor);
        let handler = proxy.handler.clone();
        let list = vec![inner.clone(), self.trap_key(&key), object];
        let answered = self.call_value(&trap, handler, list, instruction)?;
        if !answered.is_truthy() {
            return Ok(false);
        }
        self.check_define_invariant(&inner, &key, descriptor, instruction)?;
        Ok(true)
    }
}
impl<H: HostOps + ?Sized> Vm<'_, H> {
    ///
    /// `[[DefineOwnProperty]]`, with the one conversion only the machine can run.
    ///
    /// An accepted integer-indexed definition ends in `IntegerIndexedElementSet`, which is
    /// `ToNumber` -- or `ToBigInt` -- of the descriptor's value. That is the program's own
    /// method: it can throw, and it can detach the buffer the write was aimed at, which is why
    /// the write asks again rather than trusting the index the decision handed it. Every other
    /// definition is the free function's, unchanged.
    pub(crate) fn define_own_key(
        &mut self,
        target: &Value,
        key: PropertyKeyRef<'_>,
        descriptor: &Descriptor,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        if let Value::TypedArray(view) = target
            && let Some(outcome) = indexed_definition_key(view, key, descriptor)
        {
            let view = Rc::clone(view);
            return match outcome {
                IndexedDefinition::Refused => Ok(false),
                IndexedDefinition::Accepted => Ok(true),
                IndexedDefinition::Stores(at) => {
                    let value = descriptor.value.clone().unwrap_or(Value::Undefined);
                    let element = self.as_element(view.kind, &value, instruction)?;
                    view.set(at, &element)?;
                    Ok(true)
                }
            };
        }
        // Same conversion for the definition form, and for the same reason: the descriptor's
        // value arrives as the program wrote it, and an object reduces by calling its own code.
        if matches!(target, Value::Array(_))
            && key.matches_ascii("length")
            && let Some(value) = descriptor.value.clone()
            && !matches!(value, Value::Number(_))
        {
            let number = self.as_number_argument(Some(&value), instruction)?;
            let descriptor = Descriptor {
                value: Some(Value::Number(number)),
                ..descriptor.clone()
            };
            return define_own_property_key(&self.intrinsics, target, key, &descriptor);
        }
        define_own_property_key(&self.intrinsics, target, key, descriptor)
    }

    pub(crate) fn string_constant_units(
        &self,
        index: u32,
        instruction: usize,
    ) -> Result<StringUnits, ExecutionError> {
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

    pub(crate) fn string_constant_scalar(
        &self,
        index: u32,
        instruction: usize,
    ) -> Result<String, ExecutionError> {
        let name = self.string_constant_units(index, instruction)?;
        name.to_scalar_string()
            .map_err(|_| ExecutionError::InvalidPropertyKey {
                instruction,
                value: "ill-formed string constant",
            })
    }

    /// The cell at this closure's upvalue index.
    pub(crate) fn upvalue(&self, index: u32) -> Result<Cell, ExecutionError> {
        self.function
            .as_ref()
            .and_then(|closure| closure.upvalues.get(index as usize))
            .cloned()
            .ok_or(ExecutionError::MissingUpvalue { index })
    }

    /// The source binding captured at an upvalue index, when the compiler recorded one.
    pub(crate) fn upvalue_name(&self, index: u32) -> Option<String> {
        let closure = self.function.as_ref()?;
        let proto = closure.program.functions.get(closure.function as usize)?;
        let name = match proto.upvalues.get(index as usize)? {
            Upvalue::NamedParentLocal { name, .. } | Upvalue::NamedParentUpvalue { name, .. } => {
                *name
            }
            Upvalue::ParentLocal(_) | Upvalue::ParentUpvalue(_) => return None,
        };
        match closure.program.constants.get(name as usize) {
            Some(Constant::String(name)) => Some(name.to_lossy_string()),
            _ => None,
        }
    }

    /// The cell held in a local slot, for capture.
    pub(crate) fn cell(&self, slot: usize) -> Result<Cell, ExecutionError> {
        match self.locals.get(slot) {
            Some(Value::Cell(cell)) => Ok(Rc::clone(cell)),
            // `NewCell` runs before anything can capture the slot, so a non-cell here means the
            // compiler emitted a capture of a binding it never boxed.
            Some(_) | None => Err(ExecutionError::NotACell { slot }),
        }
    }
}
