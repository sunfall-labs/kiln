use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// The disposers this stack is holding, innermost last -- and whether it has run them.
    ///
    /// Each entry is `[method, receiver, arguments]`: one call, recorded rather than performed.
    /// `use` records the value's own `[Symbol.dispose]` against the value; `adopt` and `defer`
    /// record the callback the program handed over. Keeping all three as the same shape means
    /// `dispose` has one thing to do with each and no idea which spelling produced it.
    pub(super) fn disposable_entries(
        &self,
        receiver: &Value,
        what: &str,
        flavour: Disposal,
    ) -> Result<Value, ExecutionError> {
        if let Value::Object(object) = receiver
            && let Some(found) = object.borrow().get_own(flavour.entries_key())
        {
            return Ok(found.clone());
        }
        Err(ExecutionError::TypeErrorMessage {
            message: format!("{what} called on an incompatible receiver"),
        })
    }

    /// Whether this stack has already run its disposers.
    pub(super) fn disposable_is_disposed(receiver: &Value, flavour: Disposal) -> bool {
        matches!(receiver, Value::Object(object)
            if object.borrow().get_own(flavour.done_key()).is_some_and(Value::is_truthy))
    }

    /// Record that this stack's disposers have been run, or handed away.
    pub(super) fn disposable_mark_disposed(receiver: &Value, flavour: Disposal) {
        if let Value::Object(object) = receiver {
            object
                .borrow_mut()
                .set_non_enumerable(flavour.done_key(), Value::Boolean(true));
        }
    }

    /// The brand check plus the state check every method but `dispose` starts with: a stack that
    /// has been disposed accepts nothing more, because there is no longer anything that would
    /// run what it was given.
    pub(super) fn live_disposable_entries(
        &self,
        receiver: &Value,
        what: &str,
        flavour: Disposal,
    ) -> Result<Rc<ArrayData>, ExecutionError> {
        let entries = self.disposable_entries(receiver, what, flavour)?;
        // A `ReferenceError`, not a `TypeError`: the receiver is the right kind of thing and the
        // resources it named are simply no longer there to be reached, which is exactly what a
        // reference error reports.
        if Self::disposable_is_disposed(receiver, flavour) {
            return Err(ExecutionError::ReferenceErrorMessage {
                message: format!("{what} called on an already-disposed stack"),
            });
        }
        let Value::Array(entries) = entries else {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{what} called on an incompatible receiver"),
            });
        };
        Ok(entries)
    }

    /// `DisposableStack.prototype.use(value)`, and its asynchronous namesake.
    pub(super) fn disposable_use(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
        flavour: Disposal,
    ) -> Result<Value, ExecutionError> {
        let what = format!("{}.prototype.use", flavour.type_name());
        let entries = self.live_disposable_entries(receiver, &what, flavour)?;
        let value = arguments.first().cloned().unwrap_or(Value::Undefined);
        // `null` and `undefined` are nothing to dispose rather than a broken resource, which is
        // what lets `using x = maybe()` be written without a guard. The asynchronous stack still
        // *records* one, disposer and all absent: reaching it is what makes `disposeAsync` spend
        // a tick it would otherwise have spent awaiting a real disposer.
        if value.is_nullish() {
            if flavour == Disposal::Async {
                entries
                    .borrow_mut()
                    .push(disposal(Value::Undefined, Value::Undefined, Vec::new()));
            }
            return Ok(value);
        }
        let method = self.dispose_method(&value, instruction, flavour)?;
        let Some(method) = method else {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{what}: the value has no callable {}", flavour.hint_name()),
            });
        };
        entries
            .borrow_mut()
            .push(disposal(method, value.clone(), Vec::new()));
        Ok(value)
    }

    /// `GetDisposeMethod(V, hint)`: the disposer a value carries, or nothing.
    ///
    /// An asynchronous stack asks for `[Symbol.asyncDispose]` first and settles for
    /// `[Symbol.dispose]` -- and a synchronous disposer recorded here is still *awaited*, which
    /// costs the tick the specification spends on it and is what a test counting turns sees.
    /// Either name is `GetMethod`, so one that is present and not callable is a `TypeError`
    /// rather than a reason to look at the other.
    pub(super) fn dispose_method(
        &mut self,
        value: &Value,
        instruction: usize,
        flavour: Disposal,
    ) -> Result<Option<Value>, ExecutionError> {
        if flavour == Disposal::Async {
            let asynchronous =
                self.dispose_method_named(value, WellKnown::AsyncDispose, instruction)?;
            if asynchronous.is_some() {
                return Ok(asynchronous);
            }
        }
        self.dispose_method_named(value, WellKnown::Dispose, instruction)
    }

    /// `GetMethod(V, @@name)`: absent when the property is nullish, refused when it is present
    /// and not callable.
    pub(super) fn dispose_method_named(
        &mut self,
        value: &Value,
        name: WellKnown,
        instruction: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        let found =
            self.read_computed(value, &Value::Symbol(Symbol::WellKnown(name)), instruction)?;
        if found.is_nullish() {
            return Ok(None);
        }
        if !is_callable(&found) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{} is not a function", name.description()),
            });
        }
        Ok(Some(found))
    }

    /// `DisposableStack.prototype.adopt(value, onDispose)`, and its asynchronous namesake.
    pub(super) fn disposable_adopt(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        flavour: Disposal,
    ) -> Result<Value, ExecutionError> {
        let what = format!("{}.prototype.adopt", flavour.type_name());
        let entries = self.live_disposable_entries(receiver, &what, flavour)?;
        let value = arguments.first().cloned().unwrap_or(Value::Undefined);
        let on_dispose = arguments.get(1).cloned().unwrap_or(Value::Undefined);
        if !is_callable(&on_dispose) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{what}: onDispose must be a function"),
            });
        }
        entries
            .borrow_mut()
            .push(disposal(on_dispose, Value::Undefined, vec![value.clone()]));
        Ok(value)
    }

    /// `DisposableStack.prototype.defer(onDispose)`, and its asynchronous namesake.
    pub(super) fn disposable_defer(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        flavour: Disposal,
    ) -> Result<Value, ExecutionError> {
        let what = format!("{}.prototype.defer", flavour.type_name());
        let entries = self.live_disposable_entries(receiver, &what, flavour)?;
        let on_dispose = arguments.first().cloned().unwrap_or(Value::Undefined);
        if !is_callable(&on_dispose) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{what}: onDispose must be a function"),
            });
        }
        entries
            .borrow_mut()
            .push(disposal(on_dispose, Value::Undefined, Vec::new()));
        Ok(Value::Undefined)
    }

    /// `DisposableStack.prototype.move()`: hand the disposers to a fresh stack.
    ///
    /// The original is left *disposed* rather than empty, which is the point: moving is how a
    /// function that built resources transfers responsibility for them to its caller, and the
    /// stack it built must not run them on the way out.
    pub(super) fn disposable_move(
        &mut self,
        receiver: &Value,
        flavour: Disposal,
    ) -> Result<Value, ExecutionError> {
        let what = format!("{}.prototype.move", flavour.type_name());
        let entries = self.live_disposable_entries(receiver, &what, flavour)?;
        let moved = new_disposable_stack(flavour);
        let taken = std::mem::take(&mut *entries.borrow_mut());
        if let Value::Object(object) = &moved {
            object
                .borrow_mut()
                .set_non_enumerable(flavour.entries_key(), Value::Array(ArrayData::new(taken)));
        }
        Self::disposable_mark_disposed(receiver, flavour);
        Ok(moved)
    }

    /// `DisposableStack.prototype.dispose()`, which is `[Symbol.dispose]` too.
    ///
    /// Every disposer runs, in reverse order, whatever the ones before it did -- a resource that
    /// cannot be released must not strand the ones underneath it. What is left over is what the
    /// throws add up to: one of them travels as itself, and several fold into a chain of
    /// `SuppressedError`s, each naming the throw it displaced.
    pub(super) fn disposable_dispose(
        &mut self,
        receiver: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Some(pending) = self.disposable_take(receiver, Disposal::Sync)? else {
            return Ok(Value::Undefined);
        };
        let mut thrown: Option<Value> = None;
        for entry in pending.into_iter().rev() {
            let (method, this_value, list) = Self::disposal_parts(&entry);
            let outcome = self.call_value(&method, this_value, list, instruction);
            if let Err(error) = outcome {
                let raised = self.thrown_value(error)?;
                thrown = Some(self.suppressing(raised, thrown));
            }
        }
        match thrown {
            None => Ok(Value::Undefined),
            Some(value) => {
                self.caught = Some(value);
                Err(ExecutionError::ThrownAcrossNative)
            }
        }
    }

    /// Mark this stack disposed and hand back the disposers to run, innermost first.
    ///
    /// `None` when there is nothing to do because the stack is already spent -- which is not an
    /// error: disposal is idempotent, and both spellings answer a second call with nothing.
    pub(super) fn disposable_take(
        &mut self,
        receiver: &Value,
        flavour: Disposal,
    ) -> Result<Option<Vec<Value>>, ExecutionError> {
        let what = match flavour {
            Disposal::Sync => "DisposableStack.prototype.dispose",
            Disposal::Async => "AsyncDisposableStack.prototype.disposeAsync",
        };
        let entries = self.disposable_entries(receiver, what, flavour)?;
        if Self::disposable_is_disposed(receiver, flavour) {
            return Ok(None);
        }
        Self::disposable_mark_disposed(receiver, flavour);
        let Value::Array(entries) = entries else {
            return Ok(None);
        };
        // In the order they were recorded. Both callers walk them from the *back*, because a
        // resource is released before the ones it was built on top of -- which is the whole of
        // the ordering guarantee the stack makes.
        Ok(Some(std::mem::take(&mut *entries.borrow_mut())))
    }

    /// The `[method, receiver, arguments]` triple a recorded disposal holds, taken apart.
    pub(super) fn disposal_parts(entry: &Value) -> (Value, Value, Vec<Value>) {
        let Value::Array(entry) = entry else {
            return (Value::Undefined, Value::Undefined, Vec::new());
        };
        let entry = entry.borrow();
        let list = match entry.get(2) {
            Some(Value::Array(list)) => list.borrow().clone(),
            _ => Vec::new(),
        };
        (
            entry.first().cloned().unwrap_or(Value::Undefined),
            entry.get(1).cloned().unwrap_or(Value::Undefined),
            list,
        )
    }

    /// Fold a new throw into whatever disposal has already collected.
    ///
    /// The first one travels as itself; every one after it displaces the one before, and the
    /// `SuppressedError` that results names both -- so nothing a disposer raised is lost, and
    /// the outermost failure is the one a caller's `catch` sees first.
    pub(super) fn suppressing(&mut self, raised: Value, earlier: Option<Value>) -> Value {
        match earlier {
            None => raised,
            Some(earlier) => self.construct_error(ErrorKind::Suppressed, &[raised, earlier]),
        }
    }

    /// `AsyncDisposableStack.prototype.disposeAsync()`, which is `[Symbol.asyncDispose]` too.
    ///
    /// Every disposer's result is awaited before the next one runs, so what `dispose` does in a
    /// loop this does as a state machine: each await hands control back to the microtask queue
    /// and the walk resumes from [`Vm::async_disposal_resume`], which is why what it needs
    /// between steps lives in [`AsyncDisposal`] rather than on the Rust stack.
    ///
    /// Nothing here throws to the caller. It is holding a promise and nothing else -- a receiver
    /// that is not a stack at all rejects that promise rather than raising, which is what
    /// `RequireInternalSlot` inside a promise-returning method comes to.
    pub(super) fn async_disposable_dispose(
        &mut self,
        receiver: &Value,
        instruction: usize,
    ) -> Value {
        let result = PromiseState::pending();
        let handle = Value::Promise(Rc::clone(&result));
        let receiver = receiver.clone();
        let taken = self.catching(|vm| vm.disposable_take(&receiver, Disposal::Async));
        match taken {
            Ok(None) => self.settle(&result, &Settled::Fulfilled(Value::Undefined)),
            Ok(Some(pending)) => {
                let state = Rc::new(AsyncDisposal {
                    result,
                    pending: RefCell::new(pending),
                    thrown: RefCell::new(None),
                    needs_await: std::cell::Cell::new(false),
                    has_awaited: std::cell::Cell::new(false),
                });
                self.async_disposal_advance(&state, instruction);
            }
            Err(reason) => self.settle(&result, &Settled::Rejected(reason)),
        }
        handle
    }

    /// Run disposers until one of them hands back something to await, or until there are none.
    pub(super) fn async_disposal_advance(&mut self, state: &Rc<AsyncDisposal>, instruction: usize) {
        loop {
            let next = state.pending.borrow_mut().pop();
            let Some(entry) = next else {
                // The make-up tick: a stack that held only disposer-less resources has awaited
                // nothing, and the specification spends exactly one `Await(undefined)` so that
                // it settles a turn later all the same.
                if state.needs_await.get() && !state.has_awaited.get() {
                    state.has_awaited.set(true);
                    self.async_disposal_await(state, Value::Undefined);
                    return;
                }
                let thrown = state.thrown.borrow_mut().take();
                let outcome = match thrown {
                    None => Settled::Fulfilled(Value::Undefined),
                    Some(reason) => Settled::Rejected(reason),
                };
                self.settle(&state.result, &outcome);
                return;
            };
            let (method, this_value, list) = Self::disposal_parts(&entry);
            // A resource recorded without a disposer -- `stack.use(null)` -- is nothing to call.
            // It still counts, because reaching one is what makes the tick above due.
            if !is_callable(&method) {
                state.needs_await.set(true);
                continue;
            }
            match self.catching(|vm| vm.call_value(&method, this_value, list, instruction)) {
                // Awaited even when it is plainly `undefined`: the specification spends a tick
                // here whatever the disposer returned, and a test counting turns can see it.
                Ok(value) => {
                    self.async_disposal_await(state, value);
                    return;
                }
                // A disposer that throws does not strand the ones underneath it. The throw is
                // kept and the walk continues; there is nothing to await, because the call never
                // produced a value.
                Err(reason) => self.async_disposal_record(state, reason),
            }
        }
    }

    /// `Await(value)`: hand control back to the queue and resume when it settles.
    pub(super) fn async_disposal_await(&mut self, state: &Rc<AsyncDisposal>, value: Value) {
        state.has_awaited.set(true);
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
            Sink::AsyncDisposal(Rc::clone(state)),
            false,
        );
    }

    /// One awaited disposal settling.
    pub(super) fn async_disposal_resume(
        &mut self,
        state: &Rc<AsyncDisposal>,
        outcome: Settled,
        instruction: usize,
    ) {
        if let Settled::Rejected(reason) = outcome {
            self.async_disposal_record(state, reason);
        }
        self.async_disposal_advance(state, instruction);
    }

    /// Keep what a disposer raised, folded into what the ones before it raised.
    pub(super) fn async_disposal_record(&mut self, state: &Rc<AsyncDisposal>, reason: Value) {
        let earlier = state.thrown.borrow_mut().take();
        let folded = self.suppressing(reason, earlier);
        *state.thrown.borrow_mut() = Some(folded);
    }
}
