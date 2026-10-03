use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// Run an async function's body until it awaits or finishes.
    ///
    /// The body is a generator: each `await` is a suspension, and this is what resumes it. The
    /// promise the caller holds is settled when the body returns or throws, so the whole of
    /// `async`/`await` is a generator plus this loop.
    pub(crate) fn drive_async(
        &mut self,
        generator: &Rc<GeneratorData>,
        promise: &Rc<PromiseData>,
        resume: Resume,
    ) -> Result<(), ExecutionError> {
        let instruction = self.instruction;
        match self.resume_catching(generator, resume, instruction)? {
            // The body threw and nothing in it caught: that is the function's rejection -- with
            // the one exception that is not a thrown value at all. See the same guard in
            // `drive_async_generator`.
            Err(Value::Native(Native::ReturnSignal, _)) => {
                self.resolve_promise(promise, Value::Undefined);
            }
            Err(thrown) => self.settle(promise, &Settled::Rejected(thrown)),
            Ok((value, true)) => self.resolve_promise(promise, value),
            Ok((awaited, false)) => {
                // The same fast path `Promise.resolve` takes: an awaited promise is waited on
                // directly rather than wrapped in another. Wrapping would cost two extra ticks,
                // and `await` is one of the places where a tick is plainly observable.
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
                    Sink::Async(Rc::clone(generator), Rc::clone(promise)),
                    false,
                );
            }
        }
        Ok(())
    }

    /// Enter a module body, arming a resumption if it stops at a top-level `await`.
    ///
    /// A module that never awaits runs exactly as it did before: one `run`, one completion. One
    /// that does becomes an async evaluation, and the value this returns is no longer what the
    /// body produced -- the body has not produced anything yet.
    /// What one module-scope slot holds, whether or not the binding it names is boxed.
    ///
    /// A missing slot reads as the dead-zone marker rather than as `undefined`, which is the
    /// truthful answer: the binding is not there.
    pub(crate) fn module_slot_value(&self, slot: u32) -> Value {
        match self.locals.get(slot as usize) {
            Some(Value::Cell(cell)) => read_through_cell(&Value::Cell(Rc::clone(cell))),
            Some(other) => other.clone(),
            None => Value::Native(Native::Uninitialized, RealmId::PRIMARY),
        }
    }

    /// Bring every import a module made from itself back in step with the slot behind it.
    ///
    /// A binding still in its dead zone carries the dead-zone marker across rather than
    /// `undefined`: `ns.x` before `let x` runs is a ReferenceError, and answering `undefined`
    /// would be the one outcome worse than refusing to bind the namespace at all.
    pub(crate) fn refresh_self_imports(&mut self) {
        for (cell, slot) in self.self_bindings.clone() {
            let value = self.module_slot_value(slot);
            *cell.borrow_mut() = value;
        }
        for (object, exports) in self.self_namespaces.clone() {
            for (name, source) in &exports {
                let ModuleSelfExport::Slot(slot) = source else {
                    continue;
                };
                let value = self.module_slot_value(*slot);
                object
                    .borrow_mut()
                    .refresh_module_namespace_export(name, value);
            }
        }
    }

    pub(crate) fn run_module_body(&mut self) -> Result<ExecutionResult, ExecutionError> {
        let outcome = self.run()?;
        if self.suspended {
            self.suspend_module(outcome.value, 0, 0);
            return Ok(ExecutionResult {
                value: Value::Undefined,
                steps: self.steps,
                process_exit_code: None,
            });
        }
        Ok(outcome)
    }

    /// Put the module body aside on `awaited`, to be resumed by the microtask that settles it.
    ///
    /// `stack_base` and `handler_base` say where the body's own registers begin: zero on the run
    /// that started it, and wherever the microtask drain happened to be on every resumption
    /// after that.
    pub(crate) fn suspend_module(
        &mut self,
        awaited: Value,
        stack_base: usize,
        handler_base: usize,
    ) {
        if self.module_completion.is_none() {
            let completion = PromiseState::pending();
            // Read by whoever ran the module rather than by a handler, so a module that throws
            // after its first `await` is reported as the module failing rather than as a
            // rejection nobody was waiting for. Every suspension of one body settles this same
            // promise: a module is evaluated once, however many times it stops on the way.
            completion.state.borrow_mut().handled = true;
            self.module_completion = Some(completion);
        }
        let state = Rc::new(RefCell::new(ModuleSuspension {
            stack: self.stack.split_off(stack_base),
            handlers: self.handlers.split_off(handler_base),
            instruction: self.instruction,
        }));
        self.module_suspension = Some(Rc::clone(&state));
        self.suspended = false;
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
            Sink::ModuleAwait(state),
            false,
        );
    }

    /// Resume a suspended module body with what its `await` produced.
    pub(crate) fn resume_module_body(
        &mut self,
        state: &Rc<RefCell<ModuleSuspension>>,
        outcome: Settled,
    ) -> Result<(), ExecutionError> {
        let stack_base = self.stack.len();
        let handler_base = self.handlers.len();
        let (saved_stack, saved_handlers, instruction) = {
            let mut suspension = state.borrow_mut();
            (
                std::mem::take(&mut suspension.stack),
                std::mem::take(&mut suspension.handlers),
                suspension.instruction,
            )
        };
        let completion = self
            .module_completion
            .clone()
            .unwrap_or_else(PromiseState::pending);
        // Outermost, so the body's own `try` blocks are reached first and only what none of them
        // wanted arrives here. The run that would otherwise have reported a throw returned when
        // the body first suspended, so without this the failure would escape a microtask drain
        // as a machine error rather than as the module's rejection.
        self.handlers.push(Handler {
            target: HOST_HANDLER,
            frame_depth: self.frames.len(),
            stack_height: stack_base,
            native_depth: self.native_depth,
        });
        self.stack.extend(saved_stack);
        for mut handler in saved_handlers {
            handler.stack_height += stack_base;
            // Re-recorded rather than carried: a microtask drain sits deeper on the Rust stack
            // than the run that started the body, and a handler that remembered the old depth
            // would be judged unreachable from here.
            handler.native_depth = self.native_depth;
            handler.frame_depth = self.frames.len();
            self.handlers.push(handler);
        }
        self.instruction = instruction;
        self.suspended = false;

        let result = match outcome {
            Settled::Fulfilled(value) => {
                self.stack.push(value);
                self.run()
            }
            // Raised at the `await`, so a `try` written around it in the module body catches it
            // exactly as it would inside an async function.
            Settled::Rejected(reason) => match self.unwind(reason, instruction) {
                Ok(()) => self.run(),
                Err(error) => Err(error),
            },
        };

        match result {
            Ok(execution) if self.suspended => {
                self.suspend_module(execution.value, stack_base, handler_base + 1);
                self.handlers.truncate(handler_base);
                Ok(())
            }
            Ok(execution) => {
                self.stack.truncate(stack_base);
                self.handlers.truncate(handler_base);
                self.module_suspension = None;
                self.resolve_promise(&completion, execution.value);
                Ok(())
            }
            Err(ExecutionError::CaughtByHost | ExecutionError::ThrownAcrossNative) => {
                let reason = self.caught.take().unwrap_or(Value::Undefined);
                self.stack.truncate(stack_base);
                self.handlers.truncate(handler_base);
                self.module_suspension = None;
                self.settle(&completion, &Settled::Rejected(reason));
                Ok(())
            }
            Err(error) => {
                self.stack.truncate(stack_base);
                self.handlers.truncate(handler_base);
                self.module_suspension = None;
                Err(error)
            }
        }
    }

    /// What a module's evaluation came to, once every job it queued has run.
    ///
    /// `None` for a module that never awaited, and for one whose `await` is still waiting: the
    /// first never had a completion, and the second has one nothing can settle.
    pub(crate) fn module_outcome(&self) -> Option<Settled> {
        self.module_completion
            .as_ref()
            .and_then(|completion| completion.state.borrow().settled.clone())
    }
}
