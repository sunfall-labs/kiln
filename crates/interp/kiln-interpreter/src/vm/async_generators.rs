use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    pub(super) fn delegate_async_generator_return(
        &mut self,
        generator: &Rc<RefCell<GeneratorState>>,
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
        generator: &Rc<RefCell<GeneratorState>>,
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
        generator: &Rc<RefCell<GeneratorState>>,
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
                generator.borrow_mut().done = true;
                // The `.return()` marker is not a thrown value and must never become one. It
                // reaches here when the unwind it started crossed a suspension this driver did
                // not recognise, and rejecting with it hands the program a rejection whose
                // reason is not a JavaScript value at all -- `[Function: returnSignal]`, from
                // code that had merely written `break` inside `for await`. Ending the step is
                // what the marker means, so that is what it does.
                if matches!(thrown, Value::Native(Native::ReturnSignal, _)) {
                    let result = iteration_result(Value::Undefined, true);
                    self.resolve_promise(step, result);
                } else {
                    self.settle(step, &Settled::Rejected(thrown));
                }
            }
            Ok((value, true)) => {
                let result = iteration_result(value, true);
                self.resolve_promise(step, result);
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
                    self.resolve_promise(step, result);
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
