use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    pub(crate) fn settle(&mut self, promise: &Rc<PromiseData>, outcome: &Settled) {
        let (reactions, unobserved) = {
            let mut state = promise.state.borrow_mut();
            if state.settled.is_some() {
                return;
            }
            state.settled = Some(outcome.clone());
            let unobserved = !state.handled && matches!(outcome, Settled::Rejected(_));
            (std::mem::take(&mut state.reactions), unobserved)
        };
        // HostPromiseRejectionTracker's "reject": queued for the end of this checkpoint, and
        // dropped there if anything attaches first -- `p.catch(...)` on the next line is ordinary
        // code. The `.return()` marker is an internal unwinding sentinel and never a JavaScript
        // rejection.
        if unobserved
            && let Settled::Rejected(reason) = &outcome
            && !matches!(reason, Value::Native(Native::ReturnSignal, _))
        {
            promise.state.borrow_mut().rejection = RejectionReport::Waiting;
            let entry = (Rc::clone(promise), reason.clone());
            self.with_rejections_mut(|rejections| rejections.rejected.push(entry));
        }
        for reaction in reactions {
            // The reaction's own context, not this one: whatever settled the promise may be an
            // unrelated task, and the handler belongs to wherever it was attached.
            let scheduled = Scheduled {
                context: Rc::clone(&reaction.context),
                job: Job::React(outcome.clone(), reaction),
            };
            self.with_realm_mut(|realm| realm.queue_microtask(scheduled));
        }
    }

    /// Resolve a promise with a value, adopting it when that value is itself a promise.
    ///
    /// Adoption is what makes `then` chainable: returning a promise from a handler waits for it
    /// rather than fulfilling with the promise object. It needs no special machinery -- waiting
    /// for `inner` and settling `promise` with whatever it produces is exactly a reaction with no
    /// handlers, which passes both outcomes through unchanged.
    pub(crate) fn resolve_promise(&mut self, promise: &Rc<PromiseData>, value: Value) {
        let Value::Promise(inner) = &value else {
            // Anything with a callable `then` is adopted rather than stored. That is what makes a
            // promise from another library -- or a hand-written object with a `then` -- usable
            // wherever this realm's own promise is, and it is the whole of what "thenable" means.
            // Without it `await someThenable` handed the object back untouched, which looks like
            // a value and is a promise nobody is waiting on.
            if is_reflect_target(&value) {
                match self.catching(|vm| vm.read_property(&value, "then", vm.instruction)) {
                    // Reading `then` may run a getter, and a getter that throws rejects: the
                    // resolution has already been decided by the time anyone could catch it.
                    Err(reason) => self.settle(promise, &Settled::Rejected(reason)),
                    Ok(then) if is_callable(&then) => {
                        let scheduled =
                            self.scheduled(Job::Thenable(Rc::clone(promise), value, then));
                        self.with_realm_mut(|realm| realm.queue_microtask(scheduled));
                    }
                    Ok(_) => self.settle(promise, &Settled::Fulfilled(value)),
                }
                return;
            }
            self.settle(promise, &Settled::Fulfilled(value));
            return;
        };
        if Rc::ptr_eq(promise, inner) {
            // Waiting for itself would wait forever, so the specification makes it a TypeError.
            let error = self.construct_error(
                ErrorKind::Type,
                &[Value::String(
                    "Chaining cycle detected for promise".to_owned().into(),
                )],
            );
            self.settle(promise, &Settled::Rejected(error));
            return;
        }
        let scheduled = self.scheduled(Job::Adopt(Rc::clone(promise), Rc::clone(inner)));
        self.with_realm_mut(|realm| realm.queue_microtask(scheduled));
    }

    /// Attach a reaction to a promise, queueing it immediately if the promise has already settled.
    ///
    /// Queued rather than run, even when the outcome is already known: a handler must never run
    /// synchronously from `then`, or the order of a program would depend on *when* a promise
    /// settled rather than on what it does.
    pub(crate) fn react(
        &mut self,
        promise: &Rc<PromiseData>,
        on_fulfilled: Value,
        on_rejected: Value,
        sink: Sink,
        passthrough: bool,
    ) {
        let reaction = Reaction {
            on_fulfilled,
            on_rejected,
            sink,
            passthrough,
            context: Rc::clone(&self.async_context),
        };
        let (settled, handled_late) = {
            let mut state = promise.state.borrow_mut();
            state.handled = true;
            // HostPromiseRejectionTracker's "handle". A rejection still waiting for its
            // checkpoint is simply no longer reported; one already reported is `rejectionHandled`.
            let handled_late = std::mem::take(&mut state.rejection) == RejectionReport::Reported;
            (state.settled.clone(), handled_late)
        };
        if handled_late {
            let promise = Rc::clone(promise);
            self.with_rejections_mut(|rejections| rejections.handled.push(promise));
        }
        match settled {
            Some(outcome) => {
                let scheduled = Scheduled {
                    context: Rc::clone(&reaction.context),
                    job: Job::React(outcome, reaction),
                };
                self.with_realm_mut(|realm| realm.queue_microtask(scheduled));
            }
            None => promise.state.borrow_mut().reactions.push(reaction),
        }
    }

    /// Run one reaction: call its handler, and settle the promise it derives from the result.
    ///
    /// A handler that throws rejects the derived promise rather than failing the program -- which
    /// is the whole reason `call_catching` exists, since a rejection carries the thrown value.
    pub(crate) fn run_reaction(
        &mut self,
        outcome: Settled,
        reaction: Reaction,
    ) -> Result<(), ExecutionError> {
        // Neither of these carries handlers: the bookkeeping *is* the reaction.
        match &reaction.sink {
            Sink::AsyncDisposal(state) => {
                let state = Rc::clone(state);
                self.async_disposal_resume(&state, outcome, self.instruction);
                return Ok(());
            }
            Sink::AsyncGeneratorYield(generator, step) => {
                let (generator, step) = (Rc::clone(generator), Rc::clone(step));
                return match outcome {
                    Settled::Fulfilled(value) => {
                        let result = iteration_result(value, false);
                        self.settle(&step, &Settled::Fulfilled(result));
                        Ok(())
                    }
                    // The `?` on the specification's `Await` puts the failure back inside the
                    // body, at the `yield` -- so the generator's own `try` blocks see it, which
                    // settling the consumer's promise directly would deny them. Clear the delegate
                    // first so it unwinds the generator body rather than invoking inner.throw().
                    Settled::Rejected(reason) => {
                        generator.borrow_mut().delegate = None;
                        self.drive_async_generator(&generator, &step, Resume::Throw(reason))
                    }
                };
            }
            Sink::AsyncGeneratorDelegateReturnMissing(generator, step) => {
                let (generator, step) = (Rc::clone(generator), Rc::clone(step));
                return match outcome {
                    Settled::Fulfilled(value) => {
                        self.drive_async_generator(&generator, &step, Resume::Return(value))
                    }
                    Settled::Rejected(reason) => {
                        self.drive_async_generator(&generator, &step, Resume::Throw(reason))
                    }
                };
            }
            Sink::AsyncGeneratorDelegateClose(generator, step) => {
                let (generator, step) = (Rc::clone(generator), Rc::clone(step));
                return match outcome {
                    Settled::Fulfilled(close_res) => {
                        if !is_object_type(&close_res) {
                            let err = self.construct_error(
                                ErrorKind::Type,
                                &[Value::String(
                                    format!(
                                        "Iterator result {} is not an object",
                                        close_res.to_display_string()
                                    )
                                    .into(),
                                )],
                            );
                            self.drive_async_generator(&generator, &step, Resume::Throw(err))
                        } else {
                            let err = self.construct_error(
                                ErrorKind::Type,
                                &[Value::String(
                                    "Iterator does not have a throw method".to_owned().into(),
                                )],
                            );
                            self.drive_async_generator(&generator, &step, Resume::Throw(err))
                        }
                    }
                    Settled::Rejected(reason) => {
                        self.drive_async_generator(&generator, &step, Resume::Throw(reason))
                    }
                };
            }
            Sink::AsyncGeneratorDelegateReturn(generator, step, wrapped) => {
                let (generator, step, wrapped) = (Rc::clone(generator), Rc::clone(step), *wrapped);
                let instruction = self.instruction;
                return match outcome {
                    Settled::Rejected(reason) => {
                        generator.borrow_mut().delegate = None;
                        self.drive_async_generator(&generator, &step, Resume::Throw(reason))
                    }
                    Settled::Fulfilled(inner_res) => {
                        if !is_object_type(&inner_res) {
                            generator.borrow_mut().delegate = None;
                            let err = self.construct_error(
                                ErrorKind::Type,
                                &[Value::String(
                                    format!(
                                        "Iterator result {} is not an object",
                                        inner_res.to_display_string()
                                    )
                                    .into(),
                                )],
                            );
                            return self.drive_async_generator(
                                &generator,
                                &step,
                                Resume::Throw(err),
                            );
                        }
                        let done_res = self.eval_catching(|this| {
                            this.read_property(&inner_res, "done", instruction)
                        })?;
                        let done = match done_res {
                            Ok(val) => val.is_truthy(),
                            Err(thrown) => {
                                generator.borrow_mut().delegate = None;
                                return self.drive_async_generator(
                                    &generator,
                                    &step,
                                    Resume::Throw(thrown),
                                );
                            }
                        };
                        let val_res = self.eval_catching(|this| {
                            this.read_property(&inner_res, "value", instruction)
                        })?;
                        let value = match val_res {
                            Ok(v) => v,
                            Err(thrown) => {
                                generator.borrow_mut().delegate = None;
                                return self.drive_async_generator(
                                    &generator,
                                    &step,
                                    Resume::Throw(thrown),
                                );
                            }
                        };
                        if done {
                            generator.borrow_mut().delegate = None;
                        }
                        if wrapped {
                            let target = match value {
                                Value::Promise(inner) => inner,
                                other => {
                                    let pending = PromiseState::pending();
                                    self.resolve_promise(&pending, other);
                                    pending
                                }
                            };
                            self.react(
                                &target,
                                Value::Undefined,
                                Value::Undefined,
                                Sink::AsyncGeneratorDelegateStep {
                                    generator,
                                    step,
                                    exit_target: 0,
                                    done,
                                    is_return: true,
                                },
                                false,
                            );
                            Ok(())
                        } else if done {
                            self.drive_async_generator(&generator, &step, Resume::Return(value))
                        } else {
                            let result = iteration_result(value, false);
                            self.resolve_promise(&step, result);
                            Ok(())
                        }
                    }
                };
            }
            Sink::AsyncGeneratorDelegateThrow(generator, step, exit_target, wrapped) => {
                let (generator, step, exit_target, wrapped) = (
                    Rc::clone(generator),
                    Rc::clone(step),
                    *exit_target,
                    *wrapped,
                );
                let instruction = self.instruction;
                return match outcome {
                    Settled::Rejected(reason) => {
                        generator.borrow_mut().delegate = None;
                        self.drive_async_generator(&generator, &step, Resume::Throw(reason))
                    }
                    Settled::Fulfilled(inner_res) => {
                        if !is_object_type(&inner_res) {
                            generator.borrow_mut().delegate = None;
                            let err = self.construct_error(
                                ErrorKind::Type,
                                &[Value::String(
                                    format!(
                                        "Iterator result {} is not an object",
                                        inner_res.to_display_string()
                                    )
                                    .into(),
                                )],
                            );
                            return self.drive_async_generator(
                                &generator,
                                &step,
                                Resume::Throw(err),
                            );
                        }
                        let done_res = self.eval_catching(|this| {
                            this.read_property(&inner_res, "done", instruction)
                        })?;
                        let done = match done_res {
                            Ok(val) => val.is_truthy(),
                            Err(thrown) => {
                                generator.borrow_mut().delegate = None;
                                return self.drive_async_generator(
                                    &generator,
                                    &step,
                                    Resume::Throw(thrown),
                                );
                            }
                        };
                        let val_res = self.eval_catching(|this| {
                            this.read_property(&inner_res, "value", instruction)
                        })?;
                        let value = match val_res {
                            Ok(v) => v,
                            Err(thrown) => {
                                generator.borrow_mut().delegate = None;
                                return self.drive_async_generator(
                                    &generator,
                                    &step,
                                    Resume::Throw(thrown),
                                );
                            }
                        };
                        if done {
                            generator.borrow_mut().delegate = None;
                        }
                        if wrapped {
                            let target = match value {
                                Value::Promise(inner) => inner,
                                other => {
                                    let pending = PromiseState::pending();
                                    self.resolve_promise(&pending, other);
                                    pending
                                }
                            };
                            self.react(
                                &target,
                                Value::Undefined,
                                Value::Undefined,
                                Sink::AsyncGeneratorDelegateStep {
                                    generator,
                                    step,
                                    exit_target,
                                    done,
                                    is_return: false,
                                },
                                false,
                            );
                            Ok(())
                        } else if done {
                            generator.borrow_mut().instruction = exit_target;
                            self.drive_async_generator(&generator, &step, Resume::Send(value))
                        } else {
                            let result = iteration_result(value, false);
                            self.resolve_promise(&step, result);
                            Ok(())
                        }
                    }
                };
            }
            Sink::AsyncGeneratorDelegateStep {
                generator,
                step,
                exit_target,
                done,
                is_return,
            } => {
                let (generator, step, exit_target, done, is_return) = (
                    Rc::clone(generator),
                    Rc::clone(step),
                    *exit_target,
                    *done,
                    *is_return,
                );
                return match outcome {
                    Settled::Rejected(reason) => {
                        generator.borrow_mut().delegate = None;
                        self.drive_async_generator(&generator, &step, Resume::Throw(reason))
                    }
                    Settled::Fulfilled(value) => {
                        if is_return {
                            if done {
                                self.drive_async_generator(&generator, &step, Resume::Return(value))
                            } else {
                                let result = iteration_result(value, false);
                                self.resolve_promise(&step, result);
                                Ok(())
                            }
                        } else {
                            if done {
                                generator.borrow_mut().instruction = exit_target;
                                self.drive_async_generator(&generator, &step, Resume::Send(value))
                            } else {
                                let result = iteration_result(value, false);
                                self.resolve_promise(&step, result);
                                Ok(())
                            }
                        }
                    }
                };
            }
            Sink::AsyncGeneratorReturn(step) => {
                let step = Rc::clone(step);
                match outcome {
                    Settled::Fulfilled(value) => {
                        let result = iteration_result(value, true);
                        self.settle(&step, &Settled::Fulfilled(result));
                    }
                    // A `return()` handed a rejected promise rejects the step: the value never
                    // arrived, so there is no completion to report as done.
                    Settled::Rejected(reason) => self.settle(&step, &Settled::Rejected(reason)),
                }
                return Ok(());
            }
            Sink::ModuleAwait(state) => {
                let state = Rc::clone(state);
                return self.resume_module_body(&state, outcome);
            }
            Sink::Replay(promise, recorded) => {
                let promise = Rc::clone(promise);
                // A handler that returned a rejected promise replaces the outcome; anything else
                // leaves it as it was, which is what `finally` promises.
                let outcome = match outcome {
                    Settled::Rejected(reason) => Settled::Rejected(reason),
                    Settled::Fulfilled(_) => recorded.clone(),
                };
                self.settle(&promise, &outcome);
                return Ok(());
            }
            Sink::AsyncGenerator(generator, step) => {
                let (generator, step) = (Rc::clone(generator), Rc::clone(step));
                let resume = match outcome {
                    Settled::Fulfilled(value) => Resume::Send(value),
                    Settled::Rejected(reason) => Resume::Throw(reason),
                };
                return self.drive_async_generator(&generator, &step, resume);
            }
            Sink::Async(generator, promise) => {
                let (generator, promise) = (Rc::clone(generator), Rc::clone(promise));
                let resume = match outcome {
                    // The awaited value arrives as the result of the `await`.
                    Settled::Fulfilled(value) => Resume::Send(value),
                    // A rejection is raised at the `await`, so a `try` around it catches it --
                    // which is the entire reason `await` reads like synchronous code.
                    Settled::Rejected(reason) => Resume::Throw(reason),
                };
                return self.drive_async(&generator, &promise, resume);
            }
            Sink::Promise(_) => {}
        }
        let Sink::Promise(derived) = &reaction.sink else {
            unreachable!("checked just above");
        };
        let derived = Rc::clone(derived);

        let (handler, argument) = match &outcome {
            Settled::Fulfilled(value) => (reaction.on_fulfilled.clone(), value.clone()),
            Settled::Rejected(reason) => (reaction.on_rejected.clone(), reason.clone()),
        };
        if handler.is_nullish() {
            // No handler for this outcome, so it passes through unchanged -- which is what makes
            // `.then(f)` propagate a rejection and `.catch(r)` propagate a value.
            match outcome {
                Settled::Fulfilled(value) => self.resolve_promise(&derived, value),
                Settled::Rejected(reason) => {
                    self.settle(&derived, &Settled::Rejected(reason));
                }
            }
            return Ok(());
        }
        // `finally`'s handler takes no argument and its result is discarded, so the only thing
        // it can change is whether the promise rejects -- which is why a throw is still honoured.
        let arguments = if reaction.passthrough {
            Vec::new()
        } else {
            vec![argument]
        };
        match self.call_catching(&handler, Value::Undefined, arguments, self.instruction)? {
            Ok(result) => {
                if reaction.passthrough {
                    // The specification routes a `finally` handler's result through
                    // `Promise.resolve(...).then(...)`, so the outcome it restores arrives two
                    // ticks after a plain `then` queued alongside it. That delay is observable,
                    // so it is reproduced rather than optimised away -- and it is also what lets
                    // a handler returning a promise hold the chain open.
                    let waited = PromiseState::pending();
                    self.resolve_promise(&waited, result);
                    let relay = PromiseState::pending();
                    self.react(
                        &waited,
                        Value::Undefined,
                        Value::Undefined,
                        Sink::Promise(Rc::clone(&relay)),
                        false,
                    );
                    self.react(
                        &relay,
                        Value::Undefined,
                        Value::Undefined,
                        Sink::Replay(derived, outcome),
                        false,
                    );
                } else {
                    self.resolve_promise(&derived, result);
                }
            }
            Err(thrown) => self.settle(&derived, &Settled::Rejected(thrown)),
        }
        Ok(())
    }

    /// `PerformPromiseAll` and its three siblings: one lazy walk over the inputs.
    ///
    /// Lazy, not a drain, and that is the whole difference from what stood here before. The
    /// specification reads one value, hands it to `C.resolve`, and subscribes -- so a `resolve`
    /// that throws stops the walk at the first input and closes the iterator. Draining first
    /// asked an infinite iterator for every element it had, which is not a slow answer but no
    /// answer: test262 pairs a throwing `resolve` with an iterator that never ends precisely to
    /// catch an engine that does this, and each such test cost a step-limit abort.
    ///
    /// `C` is read for real too. `Promise.all.call(NotPromise, ...)` must build its promise
    /// through `NotPromise` and route every input through `NotPromise.resolve`, which is how a
    /// subclass takes part at all.
    pub(crate) fn perform_promise_combinator(
        &mut self,
        native: Native,
        constructor: &Value,
        source: &Value,
        resolve: &Value,
        reject: &Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        // `GetPromiseResolve(C)`, read once before the walk rather than once per input: the
        // specification captures it up front, and a getter here would otherwise run per element.
        let promise_resolve = self.read_property(constructor, "resolve", instruction)?;
        if !is_callable(&promise_resolve) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Promise resolve is not a function".to_owned(),
            });
        }
        let method = self.read_property(source, WellKnown::Iterator.property_key(), instruction)?;
        if !is_callable(&method) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{} is not iterable", source.to_display_string()),
            });
        }
        let iterator = self.call_value(&method, source.clone(), Vec::new(), instruction)?;
        let next = self.read_property(&iterator, "next", instruction)?;

        let walk = Rc::new(CombinatorWalk {
            native,
            values: RefCell::new(Vec::new()),
            remaining: std::cell::Cell::new(1),
            resolve: resolve.clone(),
            reject: reject.clone(),
        });

        let mut index = 0usize;
        loop {
            // A `next` that throws leaves the iterator already done, and the specification does
            // not close one it could not step -- so this propagates without closing.
            let Some(item) = self.iterator_step(&iterator, &next, instruction)? else {
                break;
            };
            // Everything from here to the subscription is inside the iterator's close window.
            let stepped = self.subscribe_combinator_input(
                &walk,
                constructor,
                &promise_resolve,
                item,
                index,
                instruction,
            );
            if let Err(error) = stepped {
                // Best effort, and deliberately not allowed to replace the original failure:
                // what the program needs to know is why the walk stopped.
                let _ = self.close_iterator(&iterator, instruction);
                return Err(error);
            }
            index += 1;
            if index > self.limits.max_steps as usize {
                return Err(ExecutionError::StepLimitExceeded {
                    steps: index as u64,
                    limit: self.limits.max_steps,
                });
            }
        }

        // The walk itself held one count, so that an input settling before the last one is read
        // cannot resolve the whole combinator early. Releasing it here is what makes an empty
        // input settle immediately, and a fully-settled one settle on the last release instead.
        walk.remaining.set(walk.remaining.get().saturating_sub(1));
        if walk.remaining.get() == 0 {
            self.finish_combinator(&walk, instruction)?;
        }
        Ok(())
    }

    /// `[[GetOwnProperty]]` as a descriptor object, through the proxy trap when there is one.
    pub(crate) fn own_descriptor_value(
        &mut self,
        target: &Value,
        key: &Value,
        instruction: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        let descriptor = if matches!(target, Value::Proxy(_)) {
            self.proxy_descriptor(target, key, instruction)?
        } else {
            let key_units = key.property_key_units_checked()?;
            own_descriptor_key(
                &self.intrinsics,
                target,
                PropertyKeyRef::from_value(key, &key_units),
            )
            .unwrap_or(Value::Undefined)
        };
        Ok((!matches!(descriptor, Value::Undefined)).then_some(descriptor))
    }

    /// One input: `C.resolve(value).then(onFulfilled, onRejected)`.
    pub(crate) fn subscribe_combinator_input(
        &mut self,
        walk: &Rc<CombinatorWalk>,
        constructor: &Value,
        promise_resolve: &Value,
        item: Value,
        index: usize,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let next_promise = self.call_value(
            promise_resolve,
            constructor.clone(),
            vec![item],
            instruction,
        )?;
        // `race` keeps no list and no count: its reactions are the capability's own functions,
        // and the first to be called wins.
        let already_called = Rc::new(std::cell::Cell::new(false));
        let element = |rejects: bool| {
            Value::Element(Rc::new(ElementFunction {
                walk: Rc::clone(walk),
                index,
                rejects,
                already_called: Rc::clone(&already_called),
            }))
        };
        let (on_fulfilled, on_rejected) = match walk.native {
            Native::PromiseAll => (element(false), walk.reject.clone()),
            Native::PromiseAllSettled => (element(false), element(true)),
            Native::PromiseAny => (walk.resolve.clone(), element(true)),
            _ => (walk.resolve.clone(), walk.reject.clone()),
        };
        if !matches!(walk.native, Native::PromiseRace) {
            walk.values.borrow_mut().push(Value::Undefined);
            walk.remaining.set(walk.remaining.get() + 1);
        }
        let then = self.read_property(&next_promise, "then", instruction)?;
        if !is_callable(&then) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{} is not a function", then.to_display_string()),
            });
        }
        self.call_value(
            &then,
            next_promise,
            vec![on_fulfilled, on_rejected],
            instruction,
        )?;
        Ok(())
    }

    /// The last count released: settle the capability with what the walk collected.
    pub(crate) fn finish_combinator(
        &mut self,
        walk: &Rc<CombinatorWalk>,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let collected = walk.values.borrow().clone();
        match walk.native {
            // An empty `any` has no input that could succeed, and a full one has had them all
            // fail; both reject with the errors in input order.
            Native::PromiseAny => {
                let error = self.aggregate_error(collected);
                self.call_value(&walk.reject, Value::Undefined, vec![error], instruction)?;
            }
            // An empty `race` never settles at all -- there is no input to settle it, and the
            // specification says so rather than leaving it undefined.
            Native::PromiseRace => {}
            _ => {
                let values = Value::Array(ArrayData::new(collected));
                self.call_value(&walk.resolve, Value::Undefined, vec![values], instruction)?;
            }
        }
        Ok(())
    }

    /// A resolve or reject element function being called: fill its slot, and release its count.
    pub(crate) fn settle_combinator_element(
        &mut self,
        element: &Rc<ElementFunction>,
        argument: Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let walk = Rc::clone(&element.walk);
        let collected = match (walk.native, element.rejects) {
            (Native::PromiseAllSettled, false) => {
                let mut data = ObjectData::default();
                data.set("status", Value::String("fulfilled".to_owned().into()));
                data.set("value", argument);
                Value::Object(Rc::new(RefCell::new(data)))
            }
            (Native::PromiseAllSettled, true) => {
                let mut data = ObjectData::default();
                data.set("status", Value::String("rejected".to_owned().into()));
                data.set("reason", argument);
                Value::Object(Rc::new(RefCell::new(data)))
            }
            _ => argument,
        };
        if let Some(slot) = walk.values.borrow_mut().get_mut(element.index) {
            *slot = collected;
        }
        walk.remaining.set(walk.remaining.get().saturating_sub(1));
        if walk.remaining.get() == 0 {
            self.finish_combinator(&walk, instruction)?;
        }
        Ok(Value::Undefined)
    }

    pub(crate) fn promise_subclass_prototype(&mut self, receiver: &Value) -> Option<Value> {
        let Value::Function(closure) = receiver else {
            return None;
        };
        (inherited_builtin(closure) == Some(Native::PromiseConstructor))
            .then(|| prototype_object(closure))
    }

    /// `NewPromiseCapability(C)`: build a promise *through* `C` and keep the pair that settles it.
    ///
    /// The specification does not build the promise and then relabel it -- it calls `C` with an
    /// executor and takes whatever `C` hands that executor. That is the difference between a
    /// subclass whose constructor runs and one whose prototype was stamped on afterwards, and it
    /// is observable three ways: the constructor's own side effects happen, `resolve` and
    /// `reject` are the functions *it* produced, and a `C` that never calls its executor is a
    /// `TypeError` rather than a promise nothing can settle.
    ///
    /// Used by the statics for which this is the whole algorithm. `then` and the four
    /// combinators still build an intrinsic promise: they need per-element resolve functions and
    /// a `GetPromiseResolve` besides this, and honouring `this` here alone would leave them
    /// waiting on a promise no reaction ever reaches.
    pub(crate) fn new_promise_capability(
        &mut self,
        constructor: &Value,
        instruction: usize,
    ) -> Result<(Value, Value, Value), ExecutionError> {
        if !is_constructor(constructor) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{} is not a constructor", constructor.to_display_string()),
            });
        }
        let record = Rc::new(CapabilityRecord::default());
        let executor = Value::Capability(Rc::clone(&record));
        let promise = self.construct_value(
            constructor.clone(),
            vec![executor],
            constructor.clone(),
            instruction,
        )?;
        let resolve = record.resolve.borrow().clone();
        let reject = record.reject.borrow().clone();
        if !is_callable(&resolve) || !is_callable(&reject) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Promise resolve or reject function is not callable".to_owned(),
            });
        }
        Ok((promise, resolve, reject))
    }

    /// Whether a promise-producing static was reached through the intrinsic `Promise` itself.
    ///
    /// The capability walk above is observable -- it runs a constructor -- so it is taken only
    /// when there is something to observe. For `%Promise%` the executor it would call is this
    /// machine's own, and the promise it would produce is the one built directly here.
    pub(crate) fn reached_through_intrinsic_promise(receiver: &Value) -> bool {
        matches!(receiver, Value::Native(Native::PromiseConstructor, _))
    }

    /// `SpeciesConstructor(promise, %Promise%)`, reduced to the prototype the promise `then`
    /// hands back should carry.
    ///
    /// `p.then(f)` on a subclass instance answers a promise of the subclass, which is what keeps
    /// a cancellable-promise layer's own methods reachable down a chain rather than only on the
    /// first link. Every step is observable and every step is taken: `constructor` off the
    /// promise, then `@@species` off that. A library that writes `static get [Symbol.species]()
    /// { return Promise; }` -- the standard way to opt a subclass *out* of this -- gets the
    /// intrinsic back, and that is `None`.
    pub(crate) fn promise_species_prototype(
        &mut self,
        promise: &Value,
        instruction: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        // `SpeciesConstructor`, with the two refusals it makes. An absent `constructor` is the
        // default; anything that is not an object is a TypeError rather than a silent fall back
        // to `%Promise%`, and a species that is not a constructor is a TypeError rather than a
        // promise built from something that cannot construct one.
        let constructor = self.read_property(promise, "constructor", instruction)?;
        if matches!(constructor, Value::Undefined) {
            return Ok(None);
        }
        if !is_an_object(&constructor) && !is_callable(&constructor) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!(
                    "{} is not an object and cannot be a constructor",
                    constructor.to_display_string()
                ),
            });
        }
        let named = self.read_computed(
            &constructor,
            &Value::Symbol(Symbol::WellKnown(WellKnown::Species)),
            instruction,
        )?;
        let species = match named {
            Value::Null | Value::Undefined => return Ok(None),
            species if is_constructor(&species) => species,
            other => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!("{} is not a constructor", other.to_display_string()),
                });
            }
        };
        Ok(self.promise_subclass_prototype(&species))
    }

    /// Give a freshly built promise the subclass prototype its constructor asked for.
    pub(crate) fn as_promise_of(promise: Value, prototype: Option<Value>) -> Value {
        if let Some(prototype) = prototype {
            record_prototype(&promise, prototype);
        }
        promise
    }

    /// What `Promise.any` rejects with when nothing succeeded: the reasons, and a sentence.
    ///
    /// An `AggregateError`, so `e.constructor.name` answers what a program's `if (e instanceof
    /// AggregateError)` is testing for. It used to be a plain `Error` carrying an `errors`
    /// array, which is a *plausible* wrong answer -- everything a handler reads off it is right
    /// except what it is.
    ///
    /// The reasons are already a list here, so this reaches the constructor beneath the
    /// iteration: `Promise.any` has nothing to iterate and no program code to run.
    pub(crate) fn aggregate_error(&mut self, reasons: Vec<Value>) -> Value {
        self.construct_error(
            ErrorKind::Aggregate,
            &[
                Value::Array(ArrayData::new(reasons)),
                Value::String("All promises were rejected".to_owned().into()),
            ],
        )
    }

    /// Run every queued job, moving the virtual clock forward to reach each timer.
    ///
    /// This is the event loop. Microtasks drain completely before any timer runs and again after
    /// each one, which is the ordering the language specifies and the reason a promise settles
    /// before a `setTimeout(f, 0)` queued earlier.
    ///
    /// Time only moves when a timer is due, so a program that schedules an hour out finishes
    /// instantly and always in the same order. Nothing here reads the host clock.
    pub(crate) fn run_event_loop(
        &mut self,
        runtime: &mut RealmRuntime,
    ) -> Result<(), ExecutionError> {
        let result = self.run_event_loop_turns(runtime);
        // The run is over: a partial line `stdout.write` left pending is written as it is.
        self.flush_stdio();
        result
    }

    fn run_event_loop_turns(&mut self, runtime: &mut RealmRuntime) -> Result<(), ExecutionError> {
        loop {
            self.advance_host_scheduler_clock(runtime, self.instruction)?;
            self.drain_microtasks(runtime)?;
            // Before asking for a timer, because settling a wait queues reactions and those are
            // microtasks: going straight on to the timer -- or worse, to the return below --
            // would leave a resolved promise with its `then` unrun.
            if self.settle_async_waits()? {
                continue;
            }
            // Pump the runtime's I/O completions (e.g., fetch responses). The HTTP transport
            // is part of the RealmRuntime in both modes; only Node.js API surface remains
            // gated. If there's no referenced I/O, this returns None and we fall through to
            // timers, preserving deterministic behavior for pure-computation programs.
            if let Some(turn) = self.with_realm_mut(|realm| runtime.next_referenced_turn(realm)) {
                match turn {
                    RuntimeTurn::Completion(completion) => {
                        runtime.enter(|| self.settle_host_completion(completion))?;
                        while let Some(next) = runtime.take_next_ready_completion() {
                            runtime.enter(|| self.settle_host_completion(next))?;
                        }
                        continue;
                    }
                    RuntimeTurn::Protocol => {
                        // The HTTP engines queued events; they'll be delivered as completions.
                        continue;
                    }
                    RuntimeTurn::Failure(failure) => {
                        return Err(ExecutionError::Host {
                            instruction: self.instruction,
                            message: failure.to_string(),
                        });
                    }
                    RuntimeTurn::Timer(work_item) => {
                        match work_item {
                            WorkItem::Timer { work: timer, .. } => {
                                // The Realm Runtime has already rearmed an interval under the same id,
                                // so clearing it inside this callback cancels the next occurrence.
                                if let Some((callback, arguments, _)) = timer.into_call() {
                                    runtime.enter(|| {
                                        self.call_value(
                                            &callback,
                                            Value::Undefined,
                                            arguments,
                                            self.instruction,
                                        )
                                    })?;
                                }
                            }
                            WorkItem::Microtask(_) => {
                                // Microtasks are drained at the top of the loop.
                            }
                        }
                        continue;
                    }
                }
            }
            let Some(WorkItem::Timer { work: timer, .. }) =
                self.with_realm_mut(|realm| runtime.next_timer(realm))
            else {
                // This realm has nothing left, but an `Atomics.waitAsync` it made may not have
                // ended. Returning here would finish the program with a promise nobody could
                // settle -- so the agent parks until the scheduler says how the wait went, and
                // the scheduler answers rather than hanging even when nothing ever will.
                if self.async_waits.is_empty() {
                    return Ok(());
                }
                let Some(group) = self.agents.clone() else {
                    return Ok(());
                };
                if group.await_async(self.agent_id).is_err() {
                    return Err(ExecutionError::AgentTerminated);
                }
                self.checkpoint = self.next_checkpoint();
                continue;
            };
            // The Realm Runtime has already rearmed an interval under the same id, so clearing
            // it inside this callback cancels the next occurrence.
            let Some((callback, arguments, _)) = timer.into_call() else {
                continue;
            };
            runtime.enter(|| {
                self.call_value(&callback, Value::Undefined, arguments, self.instruction)
            })?;
        }
    }

    /// Node's `processPromiseRejections`: hand the rejections this checkpoint left unhandled,
    /// and the reported ones handled since, to the shared tracking
    /// (`kiln-builtins/src/rejections.ts`). Answers whether it ran, since what it ran may have
    /// queued ticks and jobs the checkpoint has to drain before asking again.
    ///
    /// Asked only once both queues are empty, not at each settlement: `Promise.reject(x)`
    /// followed by a `.catch` on the next line must not be reported, and by then every handler
    /// that was going to attach in time has.
    pub(crate) fn process_rejections(&mut self) -> Result<bool, ExecutionError> {
        let (rejected, handled, process) = self.with_rejections_mut(|rejections| {
            (
                std::mem::take(&mut rejections.rejected),
                std::mem::take(&mut rejections.handled),
                rejections.process.clone(),
            )
        });
        let mut reported = Vec::with_capacity(rejected.len() * 2);
        for (promise, reason) in rejected {
            let mut state = promise.state.borrow_mut();
            if state.rejection == RejectionReport::Waiting {
                state.rejection = RejectionReport::Reported;
                drop(state);
                reported.push(Value::Promise(promise));
                reported.push(reason);
            }
        }
        if reported.is_empty() && handled.is_empty() {
            return Ok(false);
        }
        let Some(process) = process else {
            // No tracking in this realm: the first rejection fails the run.
            return match reported.into_iter().nth(1) {
                Some(reason) => Err(self.unhandled_rejection_error(&reason)),
                None => Ok(false),
            };
        };
        let handled = handled.into_iter().map(Value::Promise).collect();
        let arguments = vec![
            Value::Array(ArrayData::new(reported)),
            Value::Array(ArrayData::new(handled)),
        ];
        match self.call_catching(&process, Value::Undefined, arguments, self.instruction)? {
            Ok(_) => Ok(true),
            Err(thrown) => Err(self.unhandled_rejection_error(&thrown)),
        }
    }

    /// A rejection nothing handled, as the uncaught exception it ends the run with. `(in
    /// promise)` says where it surfaced, which is how a Test262 run tells it from a throw.
    fn unhandled_rejection_error(&mut self, reason: &Value) -> ExecutionError {
        let trace = captured_trace(reason);
        let instruction = self.instruction;
        let described = self.describe_uncaught(reason, instruction);
        // The value, for whoever reports it (a worker's parent gets a copy of the reason).
        self.uncaught_value = Some(reason.clone());
        ExecutionError::Uncaught {
            instruction,
            message: format!("(in promise) {described}"),
            error_constructor: self.error_constructor(reason),
            trace,
        }
    }

    /// Run microtasks until none are left, including ones queued by the ones already running.
    pub(crate) fn drain_microtasks(
        &mut self,
        runtime: &mut RealmRuntime,
    ) -> Result<(), ExecutionError> {
        runtime.enter(|| self.drain_microtask_jobs())
    }

    /// The ECMAScript microtask checkpoint, independent of which infrastructure turn reached it.
    /// Tokio owns polling; this queue owns language ordering.
    ///
    /// The shape of the loop is the ordering, and it is node's. `process.nextTick` outranks the
    /// promise queue, and a tick queued by a tick joins the same pass. The promise queue is then
    /// drained *whole*, without looking back at the ticks: a microtask checkpoint runs to
    /// exhaustion and is not re-entered part way, so a `nextTick` queued from inside a promise
    /// job waits for the rest of that queue. Taking one job from each lane in turn instead --
    /// which is what this did -- ran such a tick ahead of a promise job that had been queued
    /// before it, and `Promise.resolve().then(f).then(g)` with an `f` that calls `nextTick`
    /// printed the tick between `f` and `g` where node prints it after both.
    pub(crate) fn drain_microtask_jobs(&mut self) -> Result<(), ExecutionError> {
        if std::mem::take(&mut self.esm_body_pending_checkpoint) {
            // Still inside the job that evaluated the module: finish that checkpoint first.
            while let Some(scheduled) = self.with_realm_mut(|realm| realm.pop_microtask()) {
                self.run_scheduled_job(scheduled)?;
            }
        }
        loop {
            let mut ran = false;
            while let Some(scheduled) = self.with_realm_mut(|realm| realm.pop_next_tick()) {
                ran = true;
                self.run_scheduled_job(scheduled)?;
            }
            while let Some(scheduled) = self.with_realm_mut(|realm| realm.pop_microtask()) {
                ran = true;
                self.run_scheduled_job(scheduled)?;
            }
            // Node's `processTicksAndRejections`: both queues empty, then the rejections, and
            // around again for whatever reporting them queued.
            if !ran && !self.process_rejections()? {
                return Ok(());
            }
        }
    }

    /// Run one queued job in the async context it was queued in.
    pub(crate) fn run_scheduled_job(&mut self, scheduled: Scheduled) -> Result<(), ExecutionError> {
        let Scheduled { context, job } = scheduled;
        // The context this job was queued in, in force for exactly as long as it runs. A
        // drain is a flat loop rather than a stack, so the previous one is whatever the last
        // job left and is replaced rather than restored.
        self.async_context = context;
        match job {
            Job::NextTick(callback, arguments) => self
                .call_value(&callback, Value::Undefined, arguments, self.instruction)
                .map(|_| ()),
            Job::Callback(callback, arguments) => self
                .call_value(&callback, Value::Undefined, arguments, self.instruction)
                .map(|_| ()),
            Job::React(outcome, reaction) => self.run_reaction(outcome, reaction),
            Job::Adopt(outer, inner) => {
                self.react(
                    &inner,
                    Value::Undefined,
                    Value::Undefined,
                    Sink::Promise(outer),
                    false,
                );
                Ok(())
            }
            Job::Import(promise, specifier, options, module_path) => {
                let previous = std::mem::replace(&mut self.module_path, module_path);
                let imported = self.import_module(&specifier, options.as_ref());
                self.module_path = previous;
                match imported? {
                    Ok(namespace) => self.settle(&promise, &Settled::Fulfilled(namespace)),
                    Err(thrown) => self.settle(&promise, &Settled::Rejected(thrown)),
                }
                Ok(())
            }
            Job::ShadowImport {
                promise,
                specifier,
                export,
                evaluated,
                caller,
            } => {
                let settled = self.shadow_import(&specifier, &export, evaluated, caller)?;
                self.settle(&promise, &settled);
                Ok(())
            }
            Job::Thenable(promise, resolution, then) => {
                let resolve = Value::Settler(Rc::clone(&promise), false);
                let reject = Value::Settler(Rc::clone(&promise), true);
                match self.call_catching(
                    &then,
                    resolution,
                    Vec::from([resolve, reject]),
                    self.instruction,
                )? {
                    Ok(_) => {}
                    // A `then` that throws rejects -- unless it had already settled the
                    // promise before throwing, which `settle` ignores. That ordering is the
                    // whole of the specification's "already resolved" flag.
                    Err(thrown) => self.settle(&promise, &Settled::Rejected(thrown)),
                }
                Ok(())
            }
        }
    }
}
