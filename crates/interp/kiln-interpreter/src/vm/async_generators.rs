use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    pub(super) fn enqueue_async_generator_request(
        &mut self,
        generator: &Rc<GeneratorData>,
        kind: AsyncGeneratorRequestKind,
        value: Value,
        step: Rc<PromiseData>,
    ) -> Result<(), ExecutionError> {
        let request = AsyncGeneratorRequest { kind, value, step };
        let start = {
            let mut state = generator.borrow_mut();
            let requests = state
                .async_requests
                .get_or_insert_with(|| Box::new(AsyncGeneratorRequests::default()));
            if requests.current.is_none() {
                if requests.pending.is_empty() {
                    requests.current = Some(request);
                } else {
                    requests.pending.push_back(request);
                    requests.current = requests.pending.pop_front();
                }
                requests.current_started = false;
                true
            } else {
                requests.pending.push_back(request);
                false
            }
        };
        if start {
            self.start_async_generator_request(generator)
        } else {
            Ok(())
        }
    }

    fn start_async_generator_request(
        &mut self,
        generator: &Rc<GeneratorData>,
    ) -> Result<(), ExecutionError> {
        enum StartMode {
            Drain,
            Reentrant(AsyncGeneratorRequest),
            None,
        }
        let mode = {
            let mut state = generator.borrow_mut();
            let Some(requests) = state.async_requests.as_mut() else {
                return Ok(());
            };
            if requests.current.is_none() {
                requests.current = requests.pending.pop_front();
                requests.current_started = false;
            }
            if requests.current.is_none() || requests.current_started {
                StartMode::None
            } else if requests.draining {
                if requests.resolving > 0 {
                    requests.current_started = true;
                    let request = requests.current.as_ref().expect("checked above");
                    StartMode::Reentrant(AsyncGeneratorRequest {
                        kind: request.kind,
                        value: request.value.clone(),
                        step: Rc::clone(&request.step),
                    })
                } else {
                    StartMode::None
                }
            } else {
                requests.draining = true;
                StartMode::Drain
            }
        };
        match mode {
            StartMode::None => return Ok(()),
            StartMode::Reentrant(request) => {
                return self.start_current_async_generator_request(generator, &request);
            }
            StartMode::Drain => {}
        }

        loop {
            let request = {
                let mut state = generator.borrow_mut();
                let requests = state
                    .async_requests
                    .as_mut()
                    .expect("request state remains installed while draining");
                if requests.current.is_none() {
                    requests.current = requests.pending.pop_front();
                    requests.current_started = false;
                }
                let Some(current) = requests.current.as_ref() else {
                    requests.draining = false;
                    return Ok(());
                };
                if requests.current_started {
                    // A reentrant call already handed this item to the generator body.
                    requests.draining = false;
                    return Ok(());
                }
                requests.current_started = true;
                AsyncGeneratorRequest {
                    kind: current.kind,
                    value: current.value.clone(),
                    step: Rc::clone(&current.step),
                }
            };

            let started = self.start_current_async_generator_request(generator, &request);
            if let Err(error) = started {
                if let Some(requests) = generator.borrow_mut().async_requests.as_mut() {
                    requests.draining = false;
                }
                return Err(error);
            }

            let mut state = generator.borrow_mut();
            let requests = state
                .async_requests
                .as_mut()
                .expect("request state remains installed while draining");
            let still_current = requests
                .current
                .as_ref()
                .is_some_and(|current| Rc::ptr_eq(&current.step, &request.step));
            if still_current {
                // The request suspended on a promise. Its eventual completion will restart the
                // drain after this frame has unwound.
                requests.draining = false;
                return Ok(());
            }
            // The request completed synchronously. `current` may be empty, in which case the
            // next pending request is promoted by the next loop iteration. A request activated
            // reentrantly during result resolution is already marked started and is left alone.
        }
    }

    fn start_current_async_generator_request(
        &mut self,
        generator: &Rc<GeneratorData>,
        request: &AsyncGeneratorRequest,
    ) -> Result<(), ExecutionError> {
        let (started, done) = {
            let state = generator.borrow();
            (state.started, state.done)
        };
        match request.kind {
            AsyncGeneratorRequestKind::Next => self.drive_async_generator(
                generator,
                &request.step,
                Resume::Send(request.value.clone()),
            ),
            AsyncGeneratorRequestKind::Return if started && !done => self.drive_async_generator(
                generator,
                &request.step,
                Resume::Return(request.value.clone()),
            ),
            AsyncGeneratorRequestKind::Throw if started && !done => self.drive_async_generator(
                generator,
                &request.step,
                Resume::Throw(request.value.clone()),
            ),
            AsyncGeneratorRequestKind::Return => {
                generator.borrow_mut().complete();
                let target = match request.value.clone() {
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
                    Sink::AsyncGeneratorReturn(generator.clone(), Rc::clone(&request.step)),
                    false,
                );
                Ok(())
            }
            AsyncGeneratorRequestKind::Throw => {
                generator.borrow_mut().complete();
                self.finish_async_generator_request(
                    generator,
                    &request.step,
                    Settled::Rejected(request.value.clone()),
                )
            }
        }
    }

    pub(super) fn finish_async_generator_request(
        &mut self,
        generator: &Rc<GeneratorData>,
        step: &Rc<PromiseData>,
        outcome: Settled,
    ) -> Result<(), ExecutionError> {
        {
            let mut state = generator.borrow_mut();
            let Some(requests) = state.async_requests.as_mut() else {
                return Ok(());
            };
            debug_assert!(
                requests
                    .current
                    .as_ref()
                    .is_some_and(|current| Rc::ptr_eq(&current.step, step))
            );
            // AsyncGeneratorCompleteStep removes the completed request before calling its
            // promise's resolve function. That resolve may run a user `then` getter which
            // reentrantly enqueues and starts the next request.
            requests.current = None;
            requests.current_started = false;
            requests.resolving += 1;
        }
        match &outcome {
            Settled::Fulfilled(value) => self.resolve_promise(step, value.clone()),
            Settled::Rejected(_) => self.settle(step, &outcome),
        }
        let draining = {
            let mut state = generator.borrow_mut();
            let Some(requests) = state.async_requests.as_mut() else {
                return Ok(());
            };
            debug_assert!(requests.resolving > 0);
            requests.resolving -= 1;
            requests.draining
        };
        if !draining {
            self.start_async_generator_request(generator)?;
        }
        Ok(())
    }

    pub(super) fn delegate_async_generator_return(
        &mut self,
        generator: &Rc<GeneratorData>,
        step: &Rc<PromiseData>,
        delegate: Value,
        exit_target: usize,
        wrapped: bool,
        sent: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let _ = exit_target;
        let method_res =
            self.eval_catching(|this| this.get_method(&delegate, "return", instruction))?;
        let return_method = match method_res {
            Ok(method) => method,
            Err(thrown) => {
                generator.borrow_mut().delegate = None;
                return self.drive_async_generator(generator, step, Resume::Throw(thrown));
            }
        };
        match return_method {
            None => {
                generator.borrow_mut().delegate = None;
                let target = match sent {
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
                    Sink::AsyncGeneratorDelegateReturnMissing(
                        Rc::clone(generator),
                        Rc::clone(step),
                    ),
                    false,
                );
                Ok(())
            }
            Some(return_fn) => {
                let call_res = self.call_catching(&return_fn, delegate, vec![sent], instruction)?;
                let ret_val = match call_res {
                    Ok(val) => val,
                    Err(thrown) => {
                        generator.borrow_mut().delegate = None;
                        return self.drive_async_generator(generator, step, Resume::Throw(thrown));
                    }
                };
                let target = match ret_val {
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
                    Sink::AsyncGeneratorDelegateReturn(
                        Rc::clone(generator),
                        Rc::clone(step),
                        wrapped,
                    ),
                    false,
                );
                Ok(())
            }
        }
    }

    pub(super) fn delegate_async_generator_throw(
        &mut self,
        generator: &Rc<GeneratorData>,
        step: &Rc<PromiseData>,
        delegate: Value,
        exit_target: usize,
        wrapped: bool,
        sent: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let method_res =
            self.eval_catching(|this| this.get_method(&delegate, "throw", instruction))?;
        let throw_method = match method_res {
            Ok(method) => method,
            Err(thrown) => {
                generator.borrow_mut().delegate = None;
                return self.drive_async_generator(generator, step, Resume::Throw(thrown));
            }
        };
        match throw_method {
            None => {
                generator.borrow_mut().delegate = None;
                let return_method_res =
                    self.eval_catching(|this| this.get_method(&delegate, "return", instruction))?;
                match return_method_res {
                    Ok(Some(return_fn)) => {
                        let close_res =
                            self.call_catching(&return_fn, delegate, vec![], instruction)?;
                        let ret_val = match close_res {
                            Ok(val) => val,
                            Err(thrown) => {
                                return self.drive_async_generator(
                                    generator,
                                    step,
                                    Resume::Throw(thrown),
                                );
                            }
                        };
                        let target = match ret_val {
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
                            Sink::AsyncGeneratorDelegateClose(
                                Rc::clone(generator),
                                Rc::clone(step),
                            ),
                            false,
                        );
                        Ok(())
                    }
                    Ok(None) => {
                        let err = self.construct_error(
                            ErrorKind::Type,
                            &[Value::String(
                                "Iterator does not have a throw method".to_owned().into(),
                            )],
                        );
                        self.drive_async_generator(generator, step, Resume::Throw(err))
                    }
                    Err(thrown) => {
                        self.drive_async_generator(generator, step, Resume::Throw(thrown))
                    }
                }
            }
            Some(throw_fn) => {
                let call_res = self.call_catching(&throw_fn, delegate, vec![sent], instruction)?;
                let ret_val = match call_res {
                    Ok(val) => val,
                    Err(thrown) => {
                        generator.borrow_mut().delegate = None;
                        return self.drive_async_generator(generator, step, Resume::Throw(thrown));
                    }
                };
                let target = match ret_val {
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
                    Sink::AsyncGeneratorDelegateThrow(
                        Rc::clone(generator),
                        Rc::clone(step),
                        exit_target,
                        wrapped,
                    ),
                    false,
                );
                Ok(())
            }
        }
    }

    /// Advance an async generator one consumer step, settling `step` when it stops.
    ///
    /// An async generator suspends for two reasons. An `await` is invisible to the consumer: the
    /// body is waiting on something and the step is not finished, so this re-arms itself on the
    /// awaited promise. A `yield` is the step's result. That distinction is the whole of what
    /// separates this from [`Vm::drive_async`], which has only the first kind.
    pub(super) fn drive_async_generator(
        &mut self,
        generator: &Rc<GeneratorData>,
        step: &Rc<PromiseData>,
        resume: Resume,
    ) -> Result<(), ExecutionError> {
        let instruction = self.instruction;
        let delegate = generator.borrow().delegate.clone();
        if let Some((delegate_iter, exit_target, wrapped)) = delegate {
            match resume {
                Resume::Send(_) => {}
                Resume::Return(sent) => {
                    return self.delegate_async_generator_return(
                        generator,
                        step,
                        delegate_iter,
                        exit_target,
                        wrapped,
                        sent,
                        instruction,
                    );
                }
                Resume::Throw(sent) => {
                    return self.delegate_async_generator_throw(
                        generator,
                        step,
                        delegate_iter,
                        exit_target,
                        wrapped,
                        sent,
                        instruction,
                    );
                }
            }
        }
        match self.resume_catching(generator, resume, instruction)? {
            Err(thrown) => {
                // A body that threw is finished, and a later `next` has to answer `{ done: true }`
                // rather than resuming it and throwing the same thing again -- which is what it
                // did, and what turned a caught rejection into an uncaught one two lines later.
                generator.borrow_mut().complete();
                // The `.return()` marker is not a thrown value and must never become one. It
                // reaches here when the unwind it started crossed a suspension this driver did
                // not recognise, and rejecting with it hands the program a rejection whose
                // reason is not a JavaScript value at all -- `[Function: returnSignal]`, from
                // code that had merely written `break` inside `for await`. Ending the step is
                // what the marker means, so that is what it does.
                if matches!(thrown, Value::Native(Native::ReturnSignal, _)) {
                    let result = iteration_result(Value::Undefined, true);
                    self.finish_async_generator_request(
                        generator,
                        step,
                        Settled::Fulfilled(result),
                    )?;
                } else {
                    self.finish_async_generator_request(
                        generator,
                        step,
                        Settled::Rejected(thrown),
                    )?;
                }
            }
            Ok((value, true)) => {
                let result = iteration_result(value, true);
                self.finish_async_generator_request(generator, step, Settled::Fulfilled(result))?;
            }
            Ok((iterator_result, false))
                if generator.borrow().delegate.is_some() && !generator.borrow().awaiting =>
            {
                // `yield*` suspends on the delegate's iterator-result object. The compiler
                // already consumed and validated `done` before YieldStar; an async generator
                // must expose only that result's `value`, never the result object itself.
                let value = match self.eval_catching(|this| {
                    this.read_property(&iterator_result, "value", instruction)
                })? {
                    Ok(value) => value,
                    Err(thrown) => {
                        // A value getter failure is an abrupt completion of yield*. Resume the
                        // suspended body so its catch/finally paths observe the throw exactly as
                        // they do for a delegate's next() failure. Clear the delegate first so
                        // the throw is delivered to the generator body rather than calling inner.throw().
                        generator.borrow_mut().delegate = None;
                        return self.drive_async_generator(generator, step, Resume::Throw(thrown));
                    }
                };
                let wrapped = generator
                    .borrow()
                    .delegate
                    .as_ref()
                    .map(|(_, _, wrapped)| *wrapped)
                    .unwrap_or(false);
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
                        Sink::AsyncGeneratorYield(Rc::clone(generator), Rc::clone(step)),
                        false,
                    );
                } else {
                    let result = iteration_result(value, false);
                    self.finish_async_generator_request(
                        generator,
                        step,
                        Settled::Fulfilled(result),
                    )?;
                }
            }
            Ok((value, false)) if !generator.borrow().awaiting => {
                // Awaited before it is yielded, which is the specification's order and is
                // observable twice over: a yielded promise arrives as its value, and one that
                // rejects raises at the `yield` inside the body rather than at the consumer.
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
                    Sink::AsyncGeneratorYield(Rc::clone(generator), Rc::clone(step)),
                    false,
                );
            }
            Ok((awaited, false)) => {
                // Waiting, not yielding: the same fast path `await` takes in an async function.
                let target = match awaited {
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
                    Sink::AsyncGenerator(Rc::clone(generator), Rc::clone(step)),
                    false,
                );
            }
        }
        Ok(())
    }
}
