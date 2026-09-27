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
        let stored = match kind {
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
        // `includes` takes no callback and has its own rule about when the source is closed, so
        // it does not fit the walk below. It is a `TerminalKind` all the same: what the enum
        // names is "consumes the iterator and answers a value".
        if matches!(kind, TerminalKind::Includes) {
            return self.iterator_includes(receiver, arguments, instruction);
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
                    TerminalKind::Includes => unreachable!("answered above, before the walk"),
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
}
