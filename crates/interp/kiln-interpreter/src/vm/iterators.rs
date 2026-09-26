use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// `GetIteratorDirect`: the `next` this iterator will be stepped with, read once.
    ///
    /// Once, and kept: a helper that looked `next` up again on every step would follow a program
    /// that replaced it midway, which is not what the specification describes and not what a
    /// caller building a pipeline expects.
    pub(crate) fn iterator_direct(
        &mut self,
        source: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if !is_iterator_object(source) {
            return Err(ExecutionError::NotAnIterator);
        }
        self.read_property(source, "next", instruction)
    }

    /// One step of `iterator` through the `next` it was captured with.
    pub(crate) fn iterator_step(
        &mut self,
        iterator: &Value,
        next: &Value,
        instruction: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        let step = self.call_value(next, iterator.clone(), Vec::new(), instruction)?;
        if !is_iterator_object(&step) {
            return Err(ExecutionError::IteratorResultNotAnObject);
        }
        if self.read_property(&step, "done", instruction)?.is_truthy() {
            return Ok(None);
        }
        Ok(Some(self.read_property(&step, "value", instruction)?))
    }

    /// `IteratorClose`: give the source its chance to release whatever it was holding.
    ///
    /// Deliberately an ordinary call rather than a caught one. `return` on a generator works by
    /// throwing a signal through the generator's own frames, and a handler installed around the
    /// call intercepts that signal and leaves the generator suspended -- closed in name only,
    /// which is worse than not closing it at all because nothing afterwards can tell.
    pub(crate) fn close_iterator(
        &mut self,
        iterator: &Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let method = self.read_property(iterator, "return", instruction)?;
        if method.is_nullish() {
            return Ok(());
        }
        if !is_callable(&method) {
            return Err(ExecutionError::NotCallable {
                instruction,
                value: method.type_name(),
            });
        }
        self.call_value(&method, iterator.clone(), Vec::new(), instruction)?;
        Ok(())
    }

    /// Close a source when the close is not the interesting failure.
    ///
    /// Either a throw is already in flight or one is about to be raised, and the specification
    /// keeps that one: what `return` complains about while being told the iteration is over says
    /// less than the reason it ended. The value in flight is parked for the duration, because
    /// closing is itself a call and a throw from inside `return` would otherwise take the one
    /// slot the original failure is riding in.
    pub(crate) fn close_discarding(&mut self, iterator: &Value, instruction: usize) {
        let pending = self.caught.take();
        let _ = self.close_iterator(iterator, instruction);
        self.caught = pending;
    }

    /// `%IteratorPrototype%.map` and its four lazy siblings.
    ///
    /// Nothing is walked here. The result is a new iterator holding on to this one, which it
    /// pulls a single element from each time it is stepped -- the whole point of the family, and
    /// the reason `endless.map(f).take(3)` terminates.
    pub(crate) fn iterator_lazy(
        &mut self,
        kind: HelperKind,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if !is_iterator_object(receiver) {
            return Err(ExecutionError::NotAnIterator);
        }
        let argument = arguments.first().cloned().unwrap_or(Value::Undefined);
        // The argument is settled *before* `next` is so much as read, and a bad one closes the
        // iterator on the way out. Both halves are observable: a test watching the `next` getter
        // asserts it never ran, and one watching `return` asserts it did.
        // Whether a final short window is handed on. Only `windows` asks, and only its second
        // argument answers -- but it is settled here beside the size because the specification
        // checks both before it reads `next`, and a program watching the getter can tell.
        let mut allow_partial = false;
        let stored = match kind {
            // Neither of these coerces: `chunks('2')` is a `TypeError`, not a chunk size of two.
            // The contrast with `take` and `drop` two arms down is deliberate in the
            // specification -- a size is a structural argument, not a quantity to be read out of
            // whatever the program happened to pass.
            HelperKind::Chunks | HelperKind::Windows => {
                let Value::Number(size) = argument else {
                    self.close_discarding(receiver, instruction);
                    return Err(ExecutionError::TypeErrorMessage {
                        message: format!("{} size must be a Number", kind.property_key()),
                    });
                };
                // `NaN` and both infinities fail here rather than in the range check below:
                // none of them is an integral Number, and the specification asks that question
                // first. A program cannot tell `chunks(Infinity)` apart from `chunks('x')`.
                if !size.is_finite() || size.fract() != 0.0 {
                    self.close_discarding(receiver, instruction);
                    return Err(ExecutionError::TypeErrorMessage {
                        message: format!("{} size must be an integral Number", kind.property_key()),
                    });
                }
                if !(1.0..=4_294_967_295.0).contains(&size) {
                    self.close_discarding(receiver, instruction);
                    return Err(ExecutionError::IteratorLimitOutOfRange);
                }
                if matches!(kind, HelperKind::Windows) {
                    // Two spellings and nothing else -- not even a value that would stringify to
                    // one of them. The argument names a mode, and a mode read out of an
                    // arbitrary object would be a guess.
                    match arguments.get(1) {
                        None | Some(Value::Undefined) => {}
                        Some(Value::String(mode)) if mode == "only-full" => {}
                        Some(Value::String(mode)) if mode == "allow-partial" => {
                            allow_partial = true;
                        }
                        _ => {
                            self.close_discarding(receiver, instruction);
                            return Err(ExecutionError::TypeErrorMessage {
                                message:
                                    "windows undersized must be \"only-full\" or \"allow-partial\""
                                        .to_owned(),
                            });
                        }
                    }
                }
                Value::Number(size)
            }
            HelperKind::Take | HelperKind::Drop => {
                let number = match self.as_number_argument(Some(&argument), instruction) {
                    Ok(number) if !number.is_nan() && number >= 0.0 => number,
                    Ok(_) => {
                        self.close_discarding(receiver, instruction);
                        return Err(ExecutionError::IteratorLimitOutOfRange);
                    }
                    Err(error) => {
                        self.close_discarding(receiver, instruction);
                        return Err(error);
                    }
                };
                Value::Number(number.trunc())
            }
            _ if is_callable(&argument) => argument,
            _ => {
                self.close_discarding(receiver, instruction);
                return Err(ExecutionError::NotCallable {
                    instruction,
                    value: argument.type_name(),
                });
            }
        };
        let next = self.iterator_direct(receiver, instruction)?;
        let mut helper = ObjectData::default();
        helper.set_non_enumerable(HELPER_SOURCE, receiver.clone());
        helper.set_non_enumerable(HELPER_NEXT, next);
        helper.set_non_enumerable(HELPER_ARG, stored);
        helper.set_non_enumerable(HELPER_BUFFER, Value::Array(ArrayData::new(Vec::new())));
        helper.set_non_enumerable(HELPER_PARTIAL, Value::Boolean(allow_partial));
        helper.set_non_enumerable(HELPER_COUNTER, Value::Number(0.0));
        helper.set_non_enumerable(HELPER_DONE, Value::Boolean(false));
        helper.set_non_enumerable(
            "next",
            Value::Native(Native::IteratorHelperNext(kind), RealmId::PRIMARY),
        );
        helper.set_non_enumerable(
            "return",
            Value::Native(Native::IteratorHelperReturn, RealmId::PRIMARY),
        );
        helper.set_non_enumerable(
            WellKnown::Iterator.property_key(),
            Value::Native(Native::IteratorSelf, RealmId::PRIMARY),
        );
        Ok(Value::Object(Rc::new(RefCell::new(helper))))
    }

    /// `Iterator.from(value)`.
    ///
    /// Two things it is not. It is not `Array.from`: nothing is walked, and an endless iterator
    /// passes through unchanged. And it is not a wrapper in the common case -- an iterator that
    /// already inherits from `Iterator.prototype` is handed straight back, which is what makes
    /// `Iterator.from(x) === x` true for one this realm made.
    ///
    /// A value with no `@@iterator` is taken to *be* an iterator if it has a `next`, which is the
    /// whole point of the function: it is how an object that only ever implemented the iterator
    /// protocol gets the helper methods.
    pub(crate) fn iterator_from(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let value = value.cloned().unwrap_or(Value::Undefined);
        // A string is the one primitive allowed through, and it is walked as its characters.
        if !is_reflect_target(&value) && !matches!(value, Value::String(_)) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Iterator.from called on a value that is not iterable".to_owned(),
            });
        }
        let method = self.read_property(&value, WellKnown::Iterator.property_key(), instruction)?;
        let iterator = if matches!(method, Value::Undefined | Value::Null) {
            value
        } else if is_callable(&method) {
            self.call_value(&method, value, Vec::new(), instruction)?
        } else {
            return Err(ExecutionError::NotCallable {
                instruction,
                value: method.type_name(),
            });
        };
        if !is_iterator_object(&iterator) {
            return Err(ExecutionError::NotAnIterator);
        }
        let next = self.iterator_direct(&iterator, instruction)?;
        // Already on the chain, so there is nothing to add: wrapping it would give a second
        // object with the same behaviour and a different identity.
        let table = Value::Native(
            Native::PrototypeOf(PrototypeTable::Iterator),
            RealmId::PRIMARY,
        );
        let mut chain = prototype_of(&iterator);
        for _ in 0..MAX_PROTOTYPE_DEPTH {
            if matches!(chain, Value::Null) {
                break;
            }
            if strict_equal(&chain, &table) {
                return Ok(iterator);
            }
            chain = prototype_of(&chain);
        }
        let mut wrapper = ObjectData::default();
        wrapper.set_non_enumerable(HELPER_SOURCE, iterator);
        wrapper.set_non_enumerable(HELPER_NEXT, next);
        wrapper.set_non_enumerable(
            "next",
            Value::Native(Native::IteratorWrapNext, RealmId::PRIMARY),
        );
        // The same `return` the lazy helpers use: it forwards to the wrapped iterator's, and
        // answers a finished result when there is none.
        wrapper.set_non_enumerable(
            "return",
            Value::Native(Native::IteratorHelperReturn, RealmId::PRIMARY),
        );
        // On `Iterator.prototype`, which is the entire reason a program reaches for this: the
        // wrapped iterator gets `map`, `filter` and the rest.
        wrapper.set_prototype(Value::Native(
            Native::PrototypeOf(PrototypeTable::Iterator),
            RealmId::PRIMARY,
        ));
        Ok(Value::Object(Rc::new(RefCell::new(wrapper))))
    }

    /// One step of a helper's source, latching the helper done at either kind of ending.
    ///
    /// Either kind: a helper whose source threw is as finished as one that ran out, and a second
    /// `next` on it must answer done rather than reach back into an iterator that is broken.
    pub(crate) fn helper_step(
        &mut self,
        state: &Rc<RefCell<ObjectData>>,
        instruction: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        let source = helper_slot(state, HELPER_SOURCE);
        let next = helper_slot(state, HELPER_NEXT);
        let stepped = self.iterator_step(&source, &next, instruction);
        if !matches!(stepped, Ok(Some(_))) {
            state
                .borrow_mut()
                .set_non_enumerable(HELPER_DONE, Value::Boolean(true));
        }
        stepped
    }

    /// Run a helper's callback, closing the source if it fails.
    ///
    /// A helper that has already taken an element from the source owns what happens to it: the
    /// specification closes the underlying iterator before letting the callback's error out.
    pub(crate) fn helper_call(
        &mut self,
        state: &Rc<RefCell<ObjectData>>,
        callback: &Value,
        arguments: Vec<Value>,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        match self.call_value(callback, Value::Undefined, arguments, instruction) {
            Ok(value) => Ok(value),
            Err(error) => {
                state
                    .borrow_mut()
                    .set_non_enumerable(HELPER_DONE, Value::Boolean(true));
                let source = helper_slot(state, HELPER_SOURCE);
                self.close_discarding(&source, instruction);
                Err(error)
            }
        }
    }

    /// `next` on one of the five lazy helpers.
    pub(crate) fn iterator_helper_next(
        &mut self,
        kind: HelperKind,
        receiver: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Object(state) = receiver else {
            return Err(ExecutionError::NotAnIterator);
        };
        let state = Rc::clone(state);
        if state.borrow().get_own(HELPER_SOURCE).is_none() {
            return Err(ExecutionError::NotAnIterator);
        }
        if helper_slot(&state, HELPER_DONE).is_truthy() {
            return Ok(iteration_result(Value::Undefined, true));
        }
        loop {
            match kind {
                // The count is spent *before* the source is asked, so `take(0)` never touches it
                // at all -- and once it runs out the source is closed rather than abandoned.
                HelperKind::Take => {
                    let remaining = helper_slot(&state, HELPER_ARG).as_number();
                    if remaining <= 0.0 {
                        state
                            .borrow_mut()
                            .set_non_enumerable(HELPER_DONE, Value::Boolean(true));
                        let source = helper_slot(&state, HELPER_SOURCE);
                        // Propagated, not discarded: nothing failed here, so a `return` that
                        // objects is the only thing that went wrong and the caller must hear it.
                        self.close_iterator(&source, instruction)?;
                        return Ok(iteration_result(Value::Undefined, true));
                    }
                    state
                        .borrow_mut()
                        .set_non_enumerable(HELPER_ARG, Value::Number(remaining - 1.0));
                    return Ok(match self.helper_step(&state, instruction)? {
                        Some(value) => iteration_result(value, false),
                        None => iteration_result(Value::Undefined, true),
                    });
                }
                // The skipped elements are pulled on the first `next` rather than at
                // construction: `drop` is lazy too, and building one must not advance anything.
                HelperKind::Drop => {
                    loop {
                        let remaining = helper_slot(&state, HELPER_ARG).as_number();
                        if remaining <= 0.0 {
                            break;
                        }
                        state
                            .borrow_mut()
                            .set_non_enumerable(HELPER_ARG, Value::Number(remaining - 1.0));
                        if self.helper_step(&state, instruction)?.is_none() {
                            return Ok(iteration_result(Value::Undefined, true));
                        }
                    }
                    return Ok(match self.helper_step(&state, instruction)? {
                        Some(value) => iteration_result(value, false),
                        None => iteration_result(Value::Undefined, true),
                    });
                }
                // The buffer is filled, handed on whole, and emptied. A source that runs out
                // mid-bucket still hands on what it collected: a short last chunk is the
                // remainder, and dropping it would silently lose elements the program produced.
                HelperKind::Chunks => {
                    let size = helper_slot(&state, HELPER_ARG).as_number() as usize;
                    let Value::Array(buffer) = helper_slot(&state, HELPER_BUFFER) else {
                        return Err(ExecutionError::NotAnIterator);
                    };
                    let Some(value) = self.helper_step(&state, instruction)? else {
                        let collected = std::mem::take(&mut *buffer.borrow_mut());
                        return Ok(if collected.is_empty() {
                            iteration_result(Value::Undefined, true)
                        } else {
                            iteration_result(Value::Array(ArrayData::new(collected)), false)
                        });
                    };
                    buffer.borrow_mut().push(value);
                    if buffer.borrow().len() == size {
                        let collected = std::mem::take(&mut *buffer.borrow_mut());
                        return Ok(iteration_result(
                            Value::Array(ArrayData::new(collected)),
                            false,
                        ));
                    }
                }
                // The same buffer with the other eviction rule: the oldest element leaves as the
                // newest joins, and the window is *copied* out rather than taken, because the
                // elements in it are still needed by the window after this one.
                HelperKind::Windows => {
                    let size = helper_slot(&state, HELPER_ARG).as_number() as usize;
                    let Value::Array(buffer) = helper_slot(&state, HELPER_BUFFER) else {
                        return Err(ExecutionError::NotAnIterator);
                    };
                    let Some(value) = self.helper_step(&state, instruction)? else {
                        let collected = std::mem::take(&mut *buffer.borrow_mut());
                        // A short window is handed on only when the *whole source* was shorter
                        // than one window. Anything longer has already filled the buffer, and a
                        // full buffer is never drained -- so `allow-partial` cannot produce the
                        // shrinking tail a reader might expect it to.
                        if allow_partial(&state) && !collected.is_empty() && collected.len() < size
                        {
                            return Ok(iteration_result(
                                Value::Array(ArrayData::new(collected)),
                                false,
                            ));
                        }
                        return Ok(iteration_result(Value::Undefined, true));
                    };
                    {
                        let mut held = buffer.borrow_mut();
                        if held.len() == size {
                            held.remove(0);
                        }
                        held.push(value);
                    }
                    if buffer.borrow().len() == size {
                        let window = buffer.borrow().clone();
                        return Ok(iteration_result(
                            Value::Array(ArrayData::new(window)),
                            false,
                        ));
                    }
                }
                HelperKind::Map | HelperKind::Filter => {
                    let Some(value) = self.helper_step(&state, instruction)? else {
                        return Ok(iteration_result(Value::Undefined, true));
                    };
                    let counter = helper_slot(&state, HELPER_COUNTER).as_number();
                    state
                        .borrow_mut()
                        .set_non_enumerable(HELPER_COUNTER, Value::Number(counter + 1.0));
                    let callback = helper_slot(&state, HELPER_ARG);
                    let produced = self.helper_call(
                        &state,
                        &callback,
                        vec![value.clone(), Value::Number(counter)],
                        instruction,
                    )?;
                    if matches!(kind, HelperKind::Map) {
                        return Ok(iteration_result(produced, false));
                    }
                    if produced.is_truthy() {
                        return Ok(iteration_result(value, false));
                    }
                }
                HelperKind::FlatMap => {
                    let inner = helper_slot(&state, HELPER_INNER);
                    if !matches!(inner, Value::Undefined) {
                        let inner_next = helper_slot(&state, HELPER_INNER_NEXT);
                        match self.iterator_step(&inner, &inner_next, instruction) {
                            Ok(Some(value)) => return Ok(iteration_result(value, false)),
                            Ok(None) => {
                                state
                                    .borrow_mut()
                                    .set_non_enumerable(HELPER_INNER, Value::Undefined);
                            }
                            // The *outer* iterator is closed when an inner one fails: it is the
                            // resource this helper took and the one nothing else will release.
                            Err(error) => {
                                state
                                    .borrow_mut()
                                    .set_non_enumerable(HELPER_DONE, Value::Boolean(true));
                                let source = helper_slot(&state, HELPER_SOURCE);
                                self.close_discarding(&source, instruction);
                                return Err(error);
                            }
                        }
                        continue;
                    }
                    let Some(value) = self.helper_step(&state, instruction)? else {
                        return Ok(iteration_result(Value::Undefined, true));
                    };
                    let counter = helper_slot(&state, HELPER_COUNTER).as_number();
                    state
                        .borrow_mut()
                        .set_non_enumerable(HELPER_COUNTER, Value::Number(counter + 1.0));
                    let callback = helper_slot(&state, HELPER_ARG);
                    let mapped = self.helper_call(
                        &state,
                        &callback,
                        vec![value, Value::Number(counter)],
                        instruction,
                    )?;
                    match self.iterator_flattenable(&mapped, instruction) {
                        Ok((inner, inner_next)) => {
                            let mut state = state.borrow_mut();
                            state.set_non_enumerable(HELPER_INNER, inner);
                            state.set_non_enumerable(HELPER_INNER_NEXT, inner_next);
                        }
                        Err(error) => {
                            state
                                .borrow_mut()
                                .set_non_enumerable(HELPER_DONE, Value::Boolean(true));
                            let source = helper_slot(&state, HELPER_SOURCE);
                            self.close_discarding(&source, instruction);
                            return Err(error);
                        }
                    }
                }
            }
        }
    }

    /// `GetIteratorFlattenable`: what `flatMap`'s mapper is allowed to answer with.
    ///
    /// A primitive is refused rather than wrapped. A string *is* iterable, so accepting one
    /// would silently flatten a returned word into its characters -- which is the surprise the
    /// specification's `reject-primitives` flag exists to prevent.
    pub(crate) fn iterator_flattenable(
        &mut self,
        value: &Value,
        instruction: usize,
    ) -> Result<(Value, Value), ExecutionError> {
        if !is_iterator_object(value) {
            return Err(ExecutionError::NotAnIterator);
        }
        let method = self.read_property(value, WellKnown::Iterator.property_key(), instruction)?;
        let iterator = if method.is_nullish() {
            value.clone()
        } else {
            self.call_value(&method, value.clone(), Vec::new(), instruction)?
        };
        let next = self.iterator_direct(&iterator, instruction)?;
        Ok((iterator, next))
    }

    /// `helper.return()`: close the source and latch this helper done.
    pub(crate) fn iterator_helper_return(
        &mut self,
        receiver: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Object(state) = receiver else {
            return Err(ExecutionError::NotAnIterator);
        };
        let state = Rc::clone(state);
        let already = helper_slot(&state, HELPER_DONE).is_truthy();
        state
            .borrow_mut()
            .set_non_enumerable(HELPER_DONE, Value::Boolean(true));
        // A helper that already finished does not reach back into its source: the source closed
        // itself when it ran out, and calling `return` on it again is a second close.
        if !already {
            let source = helper_slot(&state, HELPER_SOURCE);
            self.close_iterator(&source, instruction)?;
        }
        Ok(iteration_result(Value::Undefined, true))
    }

    /// `IteratorCloseAll`: close every iterator, last opened first, keeping the first failure.
    ///
    /// Reverse order because the sources were opened left to right and closing is unwinding: the
    /// one opened last is the one held for the shortest time. The *first* failure encountered on
    /// that walk is the one that escapes and the rest are swallowed, which is the same rule a
    /// single `IteratorClose` follows when a throw is already in flight -- otherwise a `return`
    /// that objected would be able to hide the failure that started the unwind.
    pub(crate) fn close_all(
        &mut self,
        iterators: &[Value],
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let mut first = None;
        for iterator in iterators.iter().rev() {
            if let Err(error) = self.close_iterator(iterator, instruction)
                && first.is_none()
            {
                first = Some(error);
            }
        }
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Close every iterator when a failure is already in flight, keeping that failure.
    pub(crate) fn close_all_discarding(&mut self, iterators: &[Value], instruction: usize) {
        let pending = self.caught.take();
        let _ = self.close_all(iterators, instruction);
        self.caught = pending;
    }

    /// `GetIteratorFlattenable(value, reject-strings)`: the iterator, and its `next`.
    ///
    /// Accepts an iterable *or* a bare iterator, which is what lets `Iterator.zip` be handed a
    /// mix of arrays and half-consumed generators. A string primitive is refused where a String
    /// object is not: accepting the primitive would silently zip a word against its characters.
    pub(crate) fn iterator_flattenable_pair(
        &mut self,
        value: &Value,
        instruction: usize,
    ) -> Result<(Value, Value), ExecutionError> {
        if !is_reflect_target(value) {
            return Err(ExecutionError::NotAnIterator);
        }
        let method = self.read_property(value, WellKnown::Iterator.property_key(), instruction)?;
        let iterator = if method.is_nullish() {
            value.clone()
        } else if is_callable(&method) {
            self.call_value(&method, value.clone(), Vec::new(), instruction)?
        } else {
            return Err(ExecutionError::NotCallable {
                instruction,
                value: method.type_name(),
            });
        };
        if !is_reflect_target(&iterator) {
            return Err(ExecutionError::NotAnIterator);
        }
        // Deliberately not checked for callability: `GetIteratorDirect` only *reads* `next`, and
        // an object with neither `@@iterator` nor `next` is accepted here and fails later.
        let next = self.read_property(&iterator, "next", instruction)?;
        Ok((iterator, next))
    }

    /// `GetIterator(value, sync)`: the stricter opening, which requires a callable `@@iterator`.
    pub(crate) fn iterator_opened(
        &mut self,
        value: &Value,
        instruction: usize,
    ) -> Result<(Value, Value), ExecutionError> {
        let method = self.read_property(value, WellKnown::Iterator.property_key(), instruction)?;
        if !is_callable(&method) {
            return Err(ExecutionError::NotCallable {
                instruction,
                value: method.type_name(),
            });
        }
        let iterator = self.call_value(&method, value.clone(), Vec::new(), instruction)?;
        if !is_reflect_target(&iterator) {
            return Err(ExecutionError::NotAnIterator);
        }
        let next = self.read_property(&iterator, "next", instruction)?;
        Ok((iterator, next))
    }

    /// `Iterator.zip(iterables, options)` and `Iterator.zipKeyed(iterables, options)`.
    ///
    /// Wholly eager, which is the opposite of `concat` and is what the shape of the answer
    /// demands: a zipped step needs one element from *every* source, so every source has to be
    /// open before the first step can be taken. That is also why `return()` before the first
    /// `next()` closes all of them -- they were opened by the call, and nothing else will
    /// release them.
    ///
    /// The two spellings differ only in where the sources come from and what a step is shaped
    /// like: positional and an array, or keyed and a null-prototype object. Everything between
    /// -- the modes, the padding, the closing rules -- is one algorithm, which is why they are
    /// one function.
    pub(crate) fn iterator_zip(
        &mut self,
        keyed: bool,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let iterables = arguments.first().cloned().unwrap_or(Value::Undefined);
        // Ahead of the options, and observably so: a test pairs a primitive `iterables` with an
        // options bag whose getters throw, and asserts the `TypeError` rather than their error.
        if !is_reflect_target(&iterables) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Iterator.zip: iterables must be an object".to_owned(),
            });
        }
        let options = match arguments.get(1) {
            None | Some(Value::Undefined) => Value::Undefined,
            Some(value) if is_reflect_target(value) => value.clone(),
            Some(_) => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Iterator.zip: options must be an object".to_owned(),
                });
            }
        };
        let mode = match &options {
            Value::Undefined => Value::Undefined,
            options => self.read_property(options, "mode", instruction)?,
        };
        // Three exact spellings and nothing else -- not even a value that would stringify to
        // one. The argument names a mode, and a mode read out of an arbitrary object would be a
        // guess about what the program meant.
        let mode = match &mode {
            Value::Undefined => ZipMode::Shortest,
            Value::String(text) if text == "shortest" => ZipMode::Shortest,
            Value::String(text) if text == "longest" => ZipMode::Longest,
            Value::String(text) if text == "strict" => ZipMode::Strict,
            _ => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Iterator.zip: mode must be \"shortest\", \"longest\" or \"strict\""
                        .to_owned(),
                });
            }
        };
        // Read only in the one mode that can use it. In the other two there is nothing to pad --
        // the walk stops at the first source that ends -- so a `padding` the program left in the
        // bag is neither read nor validated.
        let padding_option = if matches!(mode, ZipMode::Longest) {
            let found = match &options {
                Value::Undefined => Value::Undefined,
                options => self.read_property(options, "padding", instruction)?,
            };
            if !matches!(found, Value::Undefined) && !is_reflect_target(&found) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Iterator.zip: padding must be an object".to_owned(),
                });
            }
            found
        } else {
            Value::Undefined
        };

        let mut iterators = Vec::new();
        let mut nexts = Vec::new();
        let mut keys = Vec::new();
        if keyed {
            self.collect_keyed_sources(
                &iterables,
                &mut keys,
                &mut iterators,
                &mut nexts,
                instruction,
            )?;
        } else {
            self.collect_positional_sources(&iterables, &mut iterators, &mut nexts, instruction)?;
        }

        let count = iterators.len();
        let padding = match self.collect_padding(keyed, &padding_option, &keys, count, instruction)
        {
            Ok(padding) => padding,
            Err(error) => {
                self.close_all_discarding(&iterators, instruction);
                return Err(error);
            }
        };

        let mut helper = ObjectData::default();
        // Each entry is the pair `[iterator, next]`, or `null` once that source has ended and is
        // being padded for. The two lists are kept apart on purpose: `iters` is indexed by
        // position and never shrinks, while `open` is what a close sweep walks and does.
        let entries: Vec<Value> = iterators
            .iter()
            .zip(&nexts)
            .map(|(iterator, next)| {
                Value::Array(ArrayData::new(vec![iterator.clone(), next.clone()]))
            })
            .collect();
        helper.set_non_enumerable(ZIP_ITERS, Value::Array(ArrayData::new(entries)));
        helper.set_non_enumerable(ZIP_OPEN, Value::Array(ArrayData::new(iterators)));
        helper.set_non_enumerable(ZIP_PADDING, Value::Array(ArrayData::new(padding)));
        helper.set_non_enumerable(ZIP_KEYS, Value::Array(ArrayData::new(keys)));
        helper.set_non_enumerable(ZIP_KEYED, Value::Boolean(keyed));
        helper.set_non_enumerable(ZIP_MODE, Value::Number(mode as u8 as f64));
        helper.set_non_enumerable(HELPER_DONE, Value::Boolean(false));
        helper.set_non_enumerable(
            "next",
            Value::Native(Native::IteratorZipNext, RealmId::PRIMARY),
        );
        helper.set_non_enumerable(
            "return",
            Value::Native(Native::IteratorZipReturn, RealmId::PRIMARY),
        );
        helper.set_non_enumerable(
            WellKnown::Iterator.property_key(),
            Value::Native(Native::IteratorSelf, RealmId::PRIMARY),
        );
        Ok(Value::Object(Rc::new(RefCell::new(helper))))
    }

    /// `Iterator.zip`'s sources: walk the outer iterable and open each element.
    pub(crate) fn collect_positional_sources(
        &mut self,
        iterables: &Value,
        iterators: &mut Vec<Value>,
        nexts: &mut Vec<Value>,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let (outer, outer_next) = self.iterator_opened(iterables, instruction)?;
        loop {
            let stepped = match self.iterator_step(&outer, &outer_next, instruction) {
                Ok(stepped) => stepped,
                // The outer iterator failed on its own terms, so it is not closed -- only the
                // sources this walk opened are, and this is the one place the two differ.
                Err(error) => {
                    self.close_all_discarding(iterators, instruction);
                    return Err(error);
                }
            };
            let Some(element) = stepped else {
                return Ok(());
            };
            match self.iterator_flattenable_pair(&element, instruction) {
                Ok((iterator, next)) => {
                    iterators.push(iterator);
                    nexts.push(next);
                }
                // A bad *element* is this walk's fault, so the outer iterator is closed too --
                // and last, after everything it produced.
                Err(error) => {
                    let mut all = iterators.clone();
                    all.insert(0, outer);
                    self.close_all_discarding(&all, instruction);
                    return Err(error);
                }
            }
        }
    }

    /// `Iterator.zipKeyed`'s sources: the own enumerable properties, in own-key order.
    pub(crate) fn collect_keyed_sources(
        &mut self,
        iterables: &Value,
        keys: &mut Vec<Value>,
        iterators: &mut Vec<Value>,
        nexts: &mut Vec<Value>,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        // The key list is taken once, up front, and everything after it is re-checked live: a
        // getter that deletes a later key means that key is skipped, and one that *adds* a key
        // has no effect, because the snapshot is what the walk reads from.
        for key in own_property_keys(&self.intrinsics, iterables) {
            let Some(attributes) = own_attributes(&self.intrinsics, iterables, &key) else {
                continue;
            };
            if !attributes.enumerable {
                continue;
            }
            let value = match self.read_property(iterables, &key, instruction) {
                Ok(value) => value,
                Err(error) => {
                    self.close_all_discarding(iterators, instruction);
                    return Err(error);
                }
            };
            // The one value that is skipped rather than refused. Every other non-iterable --
            // `null`, a number, a string -- is a `TypeError`, but `undefined` reads as "this key
            // is not one of the sources", which is what makes a sparse options-shaped object work.
            if matches!(value, Value::Undefined) {
                continue;
            }
            match self.iterator_flattenable_pair(&value, instruction) {
                Ok((iterator, next)) => {
                    keys.push(Value::String(key.into()));
                    iterators.push(iterator);
                    nexts.push(next);
                }
                Err(error) => {
                    self.close_all_discarding(iterators, instruction);
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    /// The value each source is padded with once it has ended, one per source.
    ///
    /// The two spellings read it in the two ways their result shape implies: positional zip
    /// *iterates* the padding, so `{}` is refused there, while keyed zip reads one property per
    /// key, so `{}` is a padding of all-undefined. Both produce a flat list of exactly
    /// `count` values, aligned with the sources.
    pub(crate) fn collect_padding(
        &mut self,
        keyed: bool,
        padding_option: &Value,
        keys: &[Value],
        count: usize,
        instruction: usize,
    ) -> Result<Vec<Value>, ExecutionError> {
        if matches!(padding_option, Value::Undefined) {
            return Ok(vec![Value::Undefined; count]);
        }
        if keyed {
            let mut padding = Vec::with_capacity(count);
            for key in keys {
                padding.push(self.read_property(
                    padding_option,
                    &key.to_display_string(),
                    instruction,
                )?);
            }
            return Ok(padding);
        }
        let (iterator, next) = self.iterator_opened(padding_option, instruction)?;
        let mut padding = Vec::with_capacity(count);
        let mut alive = true;
        for _ in 0..count {
            if alive {
                match self.iterator_step(&iterator, &next, instruction)? {
                    Some(value) => {
                        padding.push(value);
                        continue;
                    }
                    None => alive = false,
                }
            }
            padding.push(Value::Undefined);
        }
        // Closed only if it still had more to give: one that ran out closed itself, and calling
        // `return` on it again would be a second close.
        if alive {
            self.close_iterator(&iterator, instruction)?;
        }
        Ok(padding)
    }

    /// One step of `Iterator.zip` or `Iterator.zipKeyed`.
    pub(crate) fn iterator_zip_next(
        &mut self,
        receiver: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Object(state) = receiver else {
            return Err(ExecutionError::NotAnIterator);
        };
        let state = Rc::clone(state);
        if state.borrow().get_own(ZIP_ITERS).is_none() {
            return Err(ExecutionError::NotAnIterator);
        }
        if helper_slot(&state, HELPER_DONE).is_truthy() {
            return Ok(iteration_result(Value::Undefined, true));
        }
        let (Value::Array(iters), Value::Array(open), Value::Array(padding), Value::Array(keys)) = (
            helper_slot(&state, ZIP_ITERS),
            helper_slot(&state, ZIP_OPEN),
            helper_slot(&state, ZIP_PADDING),
            helper_slot(&state, ZIP_KEYS),
        ) else {
            return Err(ExecutionError::NotAnIterator);
        };
        let keyed = helper_slot(&state, ZIP_KEYED).is_truthy();
        let mode = ZipMode::from_number(helper_slot(&state, ZIP_MODE).as_number());
        let count = iters.borrow().len();
        // No sources at all is an empty sequence, not an endless one of empty results.
        if count == 0 {
            state
                .borrow_mut()
                .set_non_enumerable(HELPER_DONE, Value::Boolean(true));
            return Ok(iteration_result(Value::Undefined, true));
        }
        let finish = |state: &Rc<RefCell<ObjectData>>| {
            state
                .borrow_mut()
                .set_non_enumerable(HELPER_DONE, Value::Boolean(true));
        };
        let mut results = Vec::with_capacity(count);
        for index in 0..count {
            let entry = iters.borrow().get(index).cloned().unwrap_or(Value::Null);
            // Already ended and being padded for: only `longest` ever gets here, because the
            // other two modes stop the whole walk at the first source that ends.
            let Value::Array(entry) = entry else {
                results.push(
                    padding
                        .borrow()
                        .get(index)
                        .cloned()
                        .unwrap_or(Value::Undefined),
                );
                continue;
            };
            let (iterator, next) = {
                let entry = entry.borrow();
                (
                    entry.first().cloned().unwrap_or(Value::Undefined),
                    entry.get(1).cloned().unwrap_or(Value::Undefined),
                )
            };
            let stepped = match self.iterator_step(&iterator, &next, instruction) {
                Ok(stepped) => stepped,
                // The failing source is dropped from the sweep before it runs: it failed on its
                // own terms and must not also be told to close.
                Err(error) => {
                    finish(&state);
                    remove_open(&open, &iterator);
                    let remaining = open.borrow().clone();
                    self.close_all_discarding(&remaining, instruction);
                    return Err(error);
                }
            };
            let Some(value) = stepped else {
                remove_open(&open, &iterator);
                match mode {
                    // The shortest sequence decides the length, so the first source to end ends
                    // the walk -- and the sources after this index were not even asked this round.
                    ZipMode::Shortest => {
                        finish(&state);
                        let remaining = open.borrow().clone();
                        self.close_all(&remaining, instruction)?;
                        return Ok(iteration_result(Value::Undefined, true));
                    }
                    // Every source must end on the same round. Ending anywhere but the first
                    // position means an earlier one already produced a value this round, so the
                    // mismatch is certain and nothing more needs checking.
                    ZipMode::Strict => {
                        finish(&state);
                        if index != 0 {
                            let remaining = open.borrow().clone();
                            self.close_all_discarding(&remaining, instruction);
                            return Err(ExecutionError::TypeErrorMessage {
                                message: "Iterator.zip: strict mode requires every iterator to have the same length"
                                    .to_owned(),
                            });
                        }
                        return self.strict_tail(&iters, &open, count, instruction);
                    }
                    // The longest sequence decides the length: this source is retired and its
                    // slot answers from the padding from here on.
                    ZipMode::Longest => {
                        if open.borrow().is_empty() {
                            finish(&state);
                            return Ok(iteration_result(Value::Undefined, true));
                        }
                        iters.borrow_mut()[index] = Value::Null;
                        results.push(
                            padding
                                .borrow()
                                .get(index)
                                .cloned()
                                .unwrap_or(Value::Undefined),
                        );
                        continue;
                    }
                }
            };
            results.push(value);
        }
        let step = if keyed {
            let mut object = ObjectData::default();
            for (key, value) in keys.borrow().iter().zip(results) {
                object.set(&key.to_display_string(), value);
            }
            // A null prototype, so a key named `toString` or `constructor` is the source's value
            // and nothing else -- the object is a record of the step, not a participant.
            object.set_prototype(Value::Null);
            Value::Object(Rc::new(RefCell::new(object)))
        } else {
            Value::Array(ArrayData::new(results))
        };
        Ok(iteration_result(step, false))
    }

    /// Strict mode's check that every other source ended on this round too.
    ///
    /// `IteratorStep` rather than a step that reads the value: the question is only whether
    /// there *is* another element, and reading one would run a `value` getter the specification
    /// never runs.
    pub(crate) fn strict_tail(
        &mut self,
        iters: &Rc<ArrayData>,
        open: &Rc<ArrayData>,
        count: usize,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        for index in 1..count {
            let entry = iters.borrow().get(index).cloned().unwrap_or(Value::Null);
            let Value::Array(entry) = entry else {
                continue;
            };
            let (iterator, next) = {
                let entry = entry.borrow();
                (
                    entry.first().cloned().unwrap_or(Value::Undefined),
                    entry.get(1).cloned().unwrap_or(Value::Undefined),
                )
            };
            match self.iterator_step(&iterator, &next, instruction) {
                Ok(None) => {
                    remove_open(open, &iterator);
                }
                Ok(Some(_)) => {
                    let remaining = open.borrow().clone();
                    self.close_all_discarding(&remaining, instruction);
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Iterator.zip: strict mode requires every iterator to have the same length"
                            .to_owned(),
                    });
                }
                Err(error) => {
                    remove_open(open, &iterator);
                    let remaining = open.borrow().clone();
                    self.close_all_discarding(&remaining, instruction);
                    return Err(error);
                }
            }
        }
        Ok(iteration_result(Value::Undefined, true))
    }

    /// `return` on a zip helper: close every source that is still open, last first.
    pub(crate) fn iterator_zip_return(
        &mut self,
        receiver: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Object(state) = receiver else {
            return Err(ExecutionError::NotAnIterator);
        };
        let state = Rc::clone(state);
        let already = helper_slot(&state, HELPER_DONE).is_truthy();
        state
            .borrow_mut()
            .set_non_enumerable(HELPER_DONE, Value::Boolean(true));
        if already {
            return Ok(iteration_result(Value::Undefined, true));
        }
        let Value::Array(open) = helper_slot(&state, ZIP_OPEN) else {
            return Ok(iteration_result(Value::Undefined, true));
        };
        let remaining = std::mem::take(&mut *open.borrow_mut());
        self.close_all(&remaining, instruction)?;
        Ok(iteration_result(Value::Undefined, true))
    }

    /// `Iterator.concat(...items)`: the sequences run end to end, as one sequence.
    ///
    /// Split between eager and lazy in a way that is observable and deliberate. Every argument is
    /// *checked* now -- it must be an object with a callable `@@iterator`, and the method is read
    /// once and kept -- while no source is *opened* until the walk reaches it. That is what lets
    /// `Iterator.concat(finite, endless)` be built at all, and it is why a `return()` before the
    /// first `next()` closes nothing: there is nothing open to close.
    pub(crate) fn iterator_concat(
        &mut self,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let mut sources = Vec::with_capacity(arguments.len());
        for item in arguments {
            // Checked one argument at a time, both halves before the next argument is touched:
            // a test watching the second argument's getter asserts it never ran when the third
            // is the bad one.
            if !is_reflect_target(item) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Iterator.concat: every argument must be an object".to_owned(),
                });
            }
            let method =
                self.read_property(item, WellKnown::Iterator.property_key(), instruction)?;
            if !is_callable(&method) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Iterator.concat: every argument must be iterable".to_owned(),
                });
            }
            sources.push(Value::Array(ArrayData::new(vec![method, item.clone()])));
        }
        let mut helper = ObjectData::default();
        helper.set_non_enumerable(CONCAT_SOURCES, Value::Array(ArrayData::new(sources)));
        helper.set_non_enumerable(CONCAT_INDEX, Value::Number(0.0));
        // Undefined until the walk opens one, and back to undefined when it runs out: this slot
        // is both "what is open" and "is anything open", which is exactly what `return` asks.
        helper.set_non_enumerable(HELPER_SOURCE, Value::Undefined);
        helper.set_non_enumerable(HELPER_NEXT, Value::Undefined);
        helper.set_non_enumerable(HELPER_DONE, Value::Boolean(false));
        helper.set_non_enumerable(
            "next",
            Value::Native(Native::IteratorConcatNext, RealmId::PRIMARY),
        );
        helper.set_non_enumerable(
            "return",
            Value::Native(Native::IteratorConcatReturn, RealmId::PRIMARY),
        );
        helper.set_non_enumerable(
            WellKnown::Iterator.property_key(),
            Value::Native(Native::IteratorSelf, RealmId::PRIMARY),
        );
        Ok(Value::Object(Rc::new(RefCell::new(helper))))
    }

    /// One step of `Iterator.concat`.
    pub(crate) fn iterator_concat_next(
        &mut self,
        receiver: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Object(state) = receiver else {
            return Err(ExecutionError::NotAnIterator);
        };
        let state = Rc::clone(state);
        if state.borrow().get_own(CONCAT_SOURCES).is_none() {
            return Err(ExecutionError::NotAnIterator);
        }
        if helper_slot(&state, HELPER_DONE).is_truthy() {
            return Ok(iteration_result(Value::Undefined, true));
        }
        let finish = |state: &Rc<RefCell<ObjectData>>| {
            let mut state = state.borrow_mut();
            state.set_non_enumerable(HELPER_DONE, Value::Boolean(true));
            state.set_non_enumerable(HELPER_SOURCE, Value::Undefined);
            iteration_result(Value::Undefined, true)
        };
        loop {
            let open = helper_slot(&state, HELPER_SOURCE);
            if matches!(open, Value::Undefined) {
                let Value::Array(sources) = helper_slot(&state, CONCAT_SOURCES) else {
                    return Err(ExecutionError::NotAnIterator);
                };
                let index = helper_slot(&state, CONCAT_INDEX).as_number() as usize;
                let Some(entry) = sources.borrow().get(index).cloned() else {
                    return Ok(finish(&state));
                };
                state
                    .borrow_mut()
                    .set_non_enumerable(CONCAT_INDEX, Value::Number(index as f64 + 1.0));
                let Value::Array(entry) = entry else {
                    return Err(ExecutionError::NotAnIterator);
                };
                let (method, iterable) = {
                    let entry = entry.borrow();
                    (
                        entry.first().cloned().unwrap_or(Value::Undefined),
                        entry.get(1).cloned().unwrap_or(Value::Undefined),
                    )
                };
                let opened = self.call_value(&method, iterable, Vec::new(), instruction)?;
                // Only *now* is the result required to be an object -- which is why a bad
                // `@@iterator` shows up as a throw from `next` rather than from the call.
                if !is_reflect_target(&opened) {
                    state
                        .borrow_mut()
                        .set_non_enumerable(HELPER_DONE, Value::Boolean(true));
                    return Err(ExecutionError::NotAnIterator);
                }
                let next = self.read_property(&opened, "next", instruction)?;
                let mut state = state.borrow_mut();
                state.set_non_enumerable(HELPER_SOURCE, opened);
                state.set_non_enumerable(HELPER_NEXT, next);
                continue;
            }
            let next = helper_slot(&state, HELPER_NEXT);
            // A failed step ends the walk without closing anything: the source failed on its own
            // terms, and `concat` did not cause it.
            let stepped = match self.iterator_step(&open, &next, instruction) {
                Ok(stepped) => stepped,
                Err(error) => {
                    state
                        .borrow_mut()
                        .set_non_enumerable(HELPER_DONE, Value::Boolean(true));
                    return Err(error);
                }
            };
            match stepped {
                Some(value) => return Ok(iteration_result(value, false)),
                // Exhausted, so it closed itself: forget it and move to the next source without
                // calling `return` on the one that just ended.
                None => {
                    state
                        .borrow_mut()
                        .set_non_enumerable(HELPER_SOURCE, Value::Undefined);
                }
            }
        }
    }

    /// `return` on an `Iterator.concat` helper: close whichever source is open, if any.
    pub(crate) fn iterator_concat_return(
        &mut self,
        receiver: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Object(state) = receiver else {
            return Err(ExecutionError::NotAnIterator);
        };
        let state = Rc::clone(state);
        let open = helper_slot(&state, HELPER_SOURCE);
        {
            let mut state = state.borrow_mut();
            state.set_non_enumerable(HELPER_DONE, Value::Boolean(true));
            state.set_non_enumerable(HELPER_SOURCE, Value::Undefined);
        }
        if !matches!(open, Value::Undefined) {
            self.close_iterator(&open, instruction)?;
        }
        Ok(iteration_result(Value::Undefined, true))
    }

    /// `%IteratorPrototype%.includes(searchElement, skippedElements)`.
    ///
    /// The comparison is `SameValueZero`, so `NaN` finds itself and the two zeroes are one
    /// element -- and neither side is ever coerced, which is why an object matches only the
    /// reference it *is*. A match closes the source: the elements after the answer are being
    /// told they will not be asked for, and a `return` that objects to that is the only thing
    /// that went wrong, so it is propagated rather than swallowed.
    pub(crate) fn iterator_includes(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        // Settled before `next` is read, and a bad one closes the source on the way out --
        // the same order every argument in this family is checked in.
        let to_skip = match arguments.get(1) {
            None | Some(Value::Undefined) => 0.0,
            // Both infinities are admitted here where `chunks` refuses them: "skip everything"
            // is a coherent instruction, and the *negative* one is refused one step below as a
            // count rather than as a type.
            Some(Value::Number(number))
                if number.is_infinite() || (number.fract() == 0.0 && !number.is_nan()) =>
            {
                *number
            }
            _ => {
                self.close_discarding(receiver, instruction);
                return Err(ExecutionError::TypeErrorMessage {
                    message: "skippedElements must be an integral Number or an infinity".to_owned(),
                });
            }
        };
        if to_skip < 0.0 || (to_skip.is_finite() && to_skip > 9_007_199_254_740_991.0) {
            self.close_discarding(receiver, instruction);
            return Err(ExecutionError::IteratorLimitOutOfRange);
        }
        let next = self.iterator_direct(receiver, instruction)?;
        let search = arguments.first().cloned().unwrap_or(Value::Undefined);
        // Counted rather than looped `to_skip` times: the count may be `2**53` or an infinity,
        // and the source running out is what actually ends the skip.
        let mut skipped = 0.0;
        while skipped < to_skip {
            if self.iterator_step(receiver, &next, instruction)?.is_none() {
                return Ok(Value::Boolean(false));
            }
            skipped += 1.0;
        }
        loop {
            let Some(value) = self.iterator_step(receiver, &next, instruction)? else {
                // Exhaustion is not an early exit, so there is nothing to close: the source
                // ended on its own and `return` is not even looked up.
                return Ok(Value::Boolean(false));
            };
            if same_value_zero(&value, &search) {
                self.close_iterator(receiver, instruction)?;
                return Ok(Value::Boolean(true));
            }
        }
    }

    /// `%IteratorPrototype%.join(separator)`.
    ///
    /// Closes the source for the errors it causes itself -- a `toString` that throws, on either
    /// the separator or an element -- and for nothing else. An iterator that throws from `next`,
    /// or hands back something that is not a result object, has already failed on its own terms;
    /// closing it there would be this method disposing of a resource whose failure it did not
    /// cause, and the tests watch the `return` getter to prove it is not even consulted.
    pub(crate) fn iterator_join(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        // Coerced before `next` is read, which is observable: a `toString` that throws must
        // close the source without the `next` getter ever having run.
        let separator = match arguments.first() {
            None | Some(Value::Undefined) => ",".to_owned(),
            Some(value) => match self.as_text(value, instruction) {
                Ok(text) => text,
                Err(error) => {
                    self.close_discarding(receiver, instruction);
                    return Err(error);
                }
            },
        };
        let next = self.iterator_direct(receiver, instruction)?;
        let mut joined = String::new();
        let mut first = true;
        loop {
            let Some(value) = self.iterator_step(receiver, &next, instruction)? else {
                return Ok(Value::String(joined.into()));
            };
            if !first {
                joined.push_str(&separator);
            }
            first = false;
            // A nullish element contributes nothing but still gets its separator, which is what
            // makes `["a", null].values().join()` end in a comma rather than losing the hole.
            if value.is_nullish() {
                continue;
            }
            match self.as_text(&value, instruction) {
                Ok(text) => joined.push_str(&text),
                Err(error) => {
                    self.close_discarding(receiver, instruction);
                    return Err(error);
                }
            }
        }
    }

    /// `ToString`, running the program's own `toString` or `@@toPrimitive` where there is one.
    pub(crate) fn as_text(
        &mut self,
        value: &Value,
        instruction: usize,
    ) -> Result<String, ExecutionError> {
        let primitive = self.as_primitive(value, PrimitiveHint::String, instruction)?;
        // A symbol is the one primitive `ToString` refuses: it has a description, not a
        // spelling, and reading one as text is the mistake `String(symbol)` exists for.
        if matches!(primitive, Value::Symbol(_)) {
            return Err(ExecutionError::SymbolIsNotAString);
        }
        Ok(primitive.to_display_string())
    }

    /// The lossless `ToString` result used by symbol descriptions and registry keys.
    pub(crate) fn as_text_units(
        &mut self,
        value: &Value,
        instruction: usize,
    ) -> Result<StringUnits, ExecutionError> {
        let primitive = self.as_primitive(value, PrimitiveHint::String, instruction)?;
        if matches!(primitive, Value::Symbol(_)) {
            return Err(ExecutionError::SymbolIsNotAString);
        }
        primitive.to_string_units_checked()
    }

    /// Check and expose dynamic source as scalar UTF-8 for the parser boundary.
    ///
    /// The source compiler accepts `&str`, so silently widening malformed UTF-16 would change
    /// the program before the frontend sees it. ASCII source remains borrowed through the shared
    /// string representation; scalar non-ASCII source allocates once.
    pub(crate) fn checked_dynamic_source<'a>(
        &self,
        source: &'a StringUnits,
    ) -> Result<Cow<'a, str>, ExecutionError> {
        source
            .to_scalar_cow()
            .map_err(|_| ExecutionError::EvalUnsupported {
                message: DYNAMIC_SOURCE_UNPAIRED_SURROGATE.to_owned(),
            })
    }

    /// `%IteratorPrototype%.reduce` and the seven other methods that consume an iterator.
    pub(crate) fn iterator_terminal(
        &mut self,
        kind: TerminalKind,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if !is_iterator_object(receiver) {
            return Err(ExecutionError::NotAnIterator);
        }
        // Neither of these takes a callback, and each has its own rule about when the source is
        // closed, so neither fits the walk below. They are `TerminalKind`s all the same: what
        // the enum names is "consumes the iterator and answers a value", which both do.
        match kind {
            TerminalKind::Includes => {
                return self.iterator_includes(receiver, arguments, instruction);
            }
            TerminalKind::Join => return self.iterator_join(receiver, arguments, instruction),
            _ => {}
        }
        let callback = arguments.first().cloned().unwrap_or(Value::Undefined);
        if !matches!(kind, TerminalKind::ToArray) && !is_callable(&callback) {
            self.close_discarding(receiver, instruction);
            return Err(ExecutionError::NotCallable {
                instruction,
                value: callback.type_name(),
            });
        }
        let next = self.iterator_direct(receiver, instruction)?;
        let mut counter = 0.0;
        let mut collected = Vec::new();
        let mut accumulator = match kind {
            // No initial value means the first element is one, which is why an empty iterator
            // has no answer to give and gets a TypeError rather than `undefined`.
            TerminalKind::Reduce if arguments.len() < 2 => {
                let Some(first) = self.iterator_step(receiver, &next, instruction)? else {
                    return Err(ExecutionError::EmptyReduce);
                };
                counter = 1.0;
                first
            }
            TerminalKind::Reduce => arguments[1].clone(),
            _ => Value::Undefined,
        };
        loop {
            let Some(value) = self.iterator_step(receiver, &next, instruction)? else {
                return Ok(match kind {
                    TerminalKind::Reduce => accumulator,
                    TerminalKind::ToArray => Value::Array(ArrayData::new(collected)),
                    TerminalKind::Some => Value::Boolean(false),
                    TerminalKind::Every => Value::Boolean(true),
                    TerminalKind::Find | TerminalKind::ForEach => Value::Undefined,
                    TerminalKind::Includes | TerminalKind::Join => {
                        unreachable!("answered above, before the walk")
                    }
                });
            };
            if matches!(kind, TerminalKind::ToArray) {
                collected.push(value);
                continue;
            }
            let index = Value::Number(counter);
            counter += 1.0;
            let arguments = match kind {
                TerminalKind::Reduce => vec![accumulator.clone(), value.clone(), index],
                _ => vec![value.clone(), index],
            };
            let produced =
                match self.call_value(&callback, Value::Undefined, arguments, instruction) {
                    Ok(produced) => produced,
                    Err(error) => {
                        self.close_discarding(receiver, instruction);
                        return Err(error);
                    }
                };
            // Each of these three stops at the element that answered the question, and closing
            // the source is how the elements after it are told they will not be asked for.
            match kind {
                TerminalKind::Reduce => accumulator = produced,
                TerminalKind::Some if produced.is_truthy() => {
                    self.close_iterator(receiver, instruction)?;
                    return Ok(Value::Boolean(true));
                }
                TerminalKind::Every if !produced.is_truthy() => {
                    self.close_iterator(receiver, instruction)?;
                    return Ok(Value::Boolean(false));
                }
                TerminalKind::Find if produced.is_truthy() => {
                    self.close_iterator(receiver, instruction)?;
                    return Ok(value);
                }
                _ => {}
            }
        }
    }

    /// `Array.from(source, mapFn?)`.
    ///
    /// Anything iterable is walked through the protocol. Anything else is treated as array-like
    /// and read by index up to its `length`, which is the other half of what the specification
    /// says and what makes `Array.from({ length: 2 })` two undefineds rather than an error.
    /// `Array.fromAsync(asyncItems, mapfn, thisArg)`.
    ///
    /// The asynchronous sibling of `Array.from`, and the whole of the difference is where the
    /// awaits go: the source may be an async iterator, every element is awaited, and so is
    /// whatever the mapper returned. That makes it a state machine rather than a loop -- each
    /// await hands control back to the microtask queue and the walk resumes from
    /// [`Vm::async_resume`], which is why everything it needs between steps lives in
    /// [`FromAsync`] rather than on the Rust stack.
    ///
    /// Nothing after the promise exists can throw to the caller: it is holding a promise and
    /// nothing else, so every failure arrives as a rejection on it.
    pub(crate) fn array_from_async(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let result = PromiseState::pending();
        let handle = Value::Promise(Rc::clone(&result));
        let receiver = receiver.clone();
        let arguments = arguments.to_vec();
        let started = self
            .catching(|vm| vm.async_begin(&receiver, &arguments, &Rc::clone(&result), instruction));
        match started {
            Ok(state) => self.async_pump(&state, instruction),
            Err(reason) => self.settle(&result, &Settled::Rejected(reason)),
        }
        Ok(handle)
    }

    /// Settle what the walk will be before the first element is asked for.
    pub(crate) fn async_begin(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        result: &Rc<PromiseData>,
        instruction: usize,
    ) -> Result<Rc<FromAsync>, ExecutionError> {
        let items = arguments.first().cloned().unwrap_or(Value::Undefined);
        let mapper = arguments.get(1).cloned().unwrap_or(Value::Undefined);
        // Checked before the source is touched, and `null` is not "no mapper": only `undefined`
        // is, which is the same distinction `Array.from` draws.
        if !matches!(mapper, Value::Undefined) && !is_callable(&mapper) {
            return Err(ExecutionError::NotCallable {
                instruction,
                value: mapper.type_name(),
            });
        }
        let this_value = arguments.get(2).cloned().unwrap_or(Value::Undefined);
        if items.is_nullish() {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{} is not async iterable", items.to_display_string()),
            });
        }
        // The async protocol is preferred, and only when a value has none is the synchronous one
        // used -- wrapped, which is what makes each of *its* values awaited as well.
        let asynchronous =
            self.read_property(&items, WellKnown::AsyncIterator.property_key(), instruction)?;
        let (method, synchronous) = if asynchronous.is_nullish() {
            (
                self.read_property(&items, WellKnown::Iterator.property_key(), instruction)?,
                true,
            )
        } else {
            (asynchronous, false)
        };
        let source = if method.is_nullish() {
            // Neither protocol, so the value is walked as an array-like -- which is how
            // `Array.fromAsync({length: 2, 0: p, 1: q})` works and why a number produces an
            // empty array rather than a failure.
            let length = self.read_property(&items, "length", instruction)?;
            let length = to_length(self.as_number_argument(Some(&length), instruction)?);
            // Checked here rather than left to the allocation bound below: a length no array can
            // have is a `RangeError` the language defines, and the bound this implementation
            // imposes is a refusal no program can catch.
            if !is_constructor(receiver) && length > u32::MAX as usize {
                return Err(ExecutionError::InvalidArrayLength);
            }
            FromAsyncSource::ArrayLike {
                source: items,
                length,
            }
        } else {
            if !is_callable(&method) {
                return Err(ExecutionError::NotCallable {
                    instruction,
                    value: method.type_name(),
                });
            }
            let iterator = self.call_value(&method, items, Vec::new(), instruction)?;
            if !is_reflect_target(&iterator) {
                return Err(ExecutionError::NotAnIterator);
            }
            let next = self.read_property(&iterator, "next", instruction)?;
            FromAsyncSource::Iterator {
                iterator,
                next,
                synchronous,
            }
        };
        // `Array.fromAsync.call(C, …)` collects into a `C`, the same way `Array.from` does: the
        // method is generic over the constructor it is reached through, which is what makes it
        // inheritable by a subclass.
        let length = match &source {
            FromAsyncSource::ArrayLike { length, .. } => *length,
            FromAsyncSource::Iterator { .. } => 0,
        };
        let target = if is_constructor(receiver) {
            let arguments = match &source {
                FromAsyncSource::ArrayLike { .. } => vec![Value::Number(length as f64)],
                FromAsyncSource::Iterator { .. } => Vec::new(),
            };
            self.construct_value(receiver.clone(), arguments, receiver.clone(), instruction)?
        } else {
            Value::Array(ArrayData::new(Vec::new()))
        };
        Ok(Rc::new(FromAsync {
            result: Rc::clone(result),
            target,
            mapper,
            this_value,
            source: RefCell::new(source),
            index: std::cell::Cell::new(0),
            stage: std::cell::Cell::new(FromAsyncStage::Element),
        }))
    }

    /// Advance the walk, turning any failure into a rejection of the caller's promise.
    pub(crate) fn async_pump(&mut self, state: &Rc<FromAsync>, instruction: usize) {
        let state = Rc::clone(state);
        if let Err(reason) = self.catching(|vm| vm.async_advance(&state, instruction)) {
            self.async_fail(&state, reason, instruction);
        }
    }

    /// Ask the source for its next element and await it.
    pub(crate) fn async_advance(
        &mut self,
        state: &Rc<FromAsync>,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let source = state.source.borrow().clone();
        match source {
            FromAsyncSource::Iterator {
                iterator,
                next,
                synchronous,
            } => {
                let step = self.call_value(&next, iterator, Vec::new(), instruction)?;
                // An async iterator hands back a promise *of* the result object, so the object
                // itself is what has to be awaited; a synchronous one hands back the object and
                // it is each value that has to be.
                if !synchronous {
                    state.stage.set(FromAsyncStage::Step);
                    self.async_await(state, step);
                    return Ok(());
                }
                if !is_reflect_target(&step) {
                    return Err(ExecutionError::IteratorResultNotAnObject);
                }
                let done = self.read_property(&step, "done", instruction)?;
                if done.is_truthy() {
                    return self.async_finish(state, instruction);
                }
                let value = self.read_property(&step, "value", instruction)?;
                state.stage.set(FromAsyncStage::Element);
                self.async_await(state, value);
                Ok(())
            }
            FromAsyncSource::ArrayLike { source, length } => {
                let index = state.index.get();
                if index >= length {
                    return self.async_finish(state, instruction);
                }
                let value = self.read_property(&source, &index.to_string(), instruction)?;
                state.stage.set(FromAsyncStage::Element);
                self.async_await(state, value);
                Ok(())
            }
        }
    }

    /// `Await(value)`: hand control back to the queue and resume when it settles.
    pub(crate) fn async_await(&mut self, state: &Rc<FromAsync>, value: Value) {
        // A promise is awaited as itself rather than wrapped, which is what keeps the count of
        // ticks -- and so the count of `then` calls a test can observe -- the specification's.
        let promise = match &value {
            Value::Promise(existing) => Rc::clone(existing),
            other => {
                let promise = PromiseState::pending();
                self.resolve_promise(&promise, other.clone());
                promise
            }
        };
        self.react(
            &promise,
            Value::Undefined,
            Value::Undefined,
            Sink::FromAsync(Rc::clone(state)),
            false,
        );
    }

    /// One `await` inside `Array.fromAsync` settling.
    pub(crate) fn async_resume(
        &mut self,
        state: &Rc<FromAsync>,
        outcome: Settled,
        instruction: usize,
    ) {
        let value = match outcome {
            // A rejected element rejects the whole thing, and the iterator is released on the
            // way out: the walk opened it and nothing else will.
            Settled::Rejected(reason) => {
                self.async_fail(state, reason, instruction);
                return;
            }
            Settled::Fulfilled(value) => value,
        };
        let state = Rc::clone(state);
        if let Err(reason) = self.catching(|vm| vm.async_deliver(&state, value, instruction)) {
            self.async_fail(&state, reason, instruction);
        }
    }

    /// What an awaited value means, which depends on which await it was.
    pub(crate) fn async_deliver(
        &mut self,
        state: &Rc<FromAsync>,
        value: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        match state.stage.get() {
            FromAsyncStage::Step => {
                if !is_reflect_target(&value) {
                    return Err(ExecutionError::IteratorResultNotAnObject);
                }
                let done = self.read_property(&value, "done", instruction)?;
                if done.is_truthy() {
                    return self.async_finish(state, instruction);
                }
                // Not awaited a second time: an async iterator's protocol has already resolved
                // the value, and awaiting it again would cost a tick a test can count.
                let element = self.read_property(&value, "value", instruction)?;
                self.async_element(state, element, instruction)
            }
            FromAsyncStage::Element => self.async_element(state, value, instruction),
            FromAsyncStage::Mapped => self.async_place(state, value, instruction),
        }
    }

    /// An element has arrived: map it, or place it as it is.
    pub(crate) fn async_element(
        &mut self,
        state: &Rc<FromAsync>,
        value: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let mapper = state.mapper.clone();
        if !is_callable(&mapper) {
            return self.async_place(state, value, instruction);
        }
        let mapped = self.call_value(
            &mapper,
            state.this_value.clone(),
            Vec::from([value, Value::Number(state.index.get() as f64)]),
            instruction,
        )?;
        state.stage.set(FromAsyncStage::Mapped);
        self.async_await(state, mapped);
        Ok(())
    }

    /// Store one element and ask for the next.
    pub(crate) fn async_place(
        &mut self,
        state: &Rc<FromAsync>,
        value: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let index = state.index.get();
        // `CreateDataPropertyOrThrow`, not an assignment. Two things follow from that and both
        // are tested: a collection that *refuses* the element stops the walk, where an
        // assignment would be silently discarded and an endless iterator would never end; and
        // the definition goes through a proxy's `defineProperty` trap rather than its `set`.
        let mut descriptor = ObjectData::default();
        descriptor.set("value", value);
        descriptor.set("writable", Value::Boolean(true));
        descriptor.set("enumerable", Value::Boolean(true));
        descriptor.set("configurable", Value::Boolean(true));
        self.dispatch_native(
            Native::ObjectDefineProperty,
            &Value::Undefined,
            &[
                state.target.clone(),
                Value::String(index.to_string().into()),
                Value::Object(Rc::new(RefCell::new(descriptor))),
            ],
            instruction,
        )?;
        state.index.set(index + 1);
        self.async_advance(state, instruction)
    }

    /// The source is spent: publish the length and hand the collection over.
    pub(crate) fn async_finish(
        &mut self,
        state: &Rc<FromAsync>,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        // `Set(A, "length", k, true)`: the `true` is throw-on-failure, so a collection whose
        // `length` cannot be written refuses the whole operation rather than quietly reporting
        // a length that is not the one it holds.
        if own_attributes(&self.intrinsics, &state.target, "length")
            .is_some_and(|attributes| !attributes.writable)
        {
            return Err(ExecutionError::TypeErrorMessage {
                message: refusal(&self.intrinsics, &state.target, "length"),
            });
        }
        self.set_computed(
            &state.target,
            &Value::String("length".to_owned().into()),
            Value::Number(state.index.get() as f64),
            instruction,
        )?;
        let target = state.target.clone();
        self.resolve_promise(&state.result, target);
        Ok(())
    }

    /// Release the iterator, if there is one, and reject.
    pub(crate) fn async_fail(&mut self, state: &Rc<FromAsync>, reason: Value, instruction: usize) {
        let iterator = match &*state.source.borrow() {
            FromAsyncSource::Iterator { iterator, .. } => Some(iterator.clone()),
            FromAsyncSource::ArrayLike { .. } => None,
        };
        if let Some(iterator) = iterator {
            // Discarded: what `return` complains about while being told the walk is over cannot
            // be more interesting than the failure that ended it.
            let _ = self.catching(|vm| vm.close_iterator(&iterator, instruction));
        }
        self.settle(&state.result, &Settled::Rejected(reason));
    }
}
