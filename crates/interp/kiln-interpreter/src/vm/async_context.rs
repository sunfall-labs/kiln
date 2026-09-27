use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// The five `AsyncLocalStorage` methods, which share a receiver and a context.
    ///
    /// The store is not held on the storage. It is held in the *context*, which is realm state
    /// that follows control rather than the frame -- captured when work is queued and restored
    /// when that work runs. That is the whole mechanism, and it is why `als.run(store, f)` where
    /// `f` awaits still sees the store on the far side of the await: the reaction that resumes
    /// `f` was queued while the store was in force and carries a copy of the context that had it.
    pub(super) fn async_local_storage(
        &mut self,
        native: Native,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        // The constructor reaches here when it is called without `new`, which Node refuses.
        if matches!(native, Native::NodeAsyncLocalStorage) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Class constructor AsyncLocalStorage cannot be invoked without 'new'"
                    .into(),
            });
        }
        let storage = node_async_storage(receiver)?;
        match native {
            Native::NodeAsyncLocalStorageGetStore => Ok(self.current_store(&storage)),
            Native::NodeAsyncLocalStorageDisable => {
                // Permanent, as Node's is. A disabled storage reads as empty even from a context
                // captured while it still held a store, which is why this is a flag on the
                // storage and not a removal from a context nothing can reach any more.
                storage.disabled.set(true);
                Ok(Value::Undefined)
            }
            Native::NodeAsyncLocalStorageEnterWith => {
                // No matching exit: the store stays in force for the rest of *this* job, and
                // every job queued from here carries it. That is what distinguishes it from
                // `run`, which restores what it displaced.
                let store = arguments.first().cloned().unwrap_or(Value::Undefined);
                self.async_context = bind_store(&self.async_context, &storage, Some(store));
                Ok(Value::Undefined)
            }
            // Both run a callback under a modified context and put back exactly what was there,
            // whether the callback returned or threw. `run` binds a store; `exit` removes one.
            Native::NodeAsyncLocalStorageRun | Native::NodeAsyncLocalStorageExit => {
                let exiting = matches!(native, Native::NodeAsyncLocalStorageExit);
                let (callback, rest) = if exiting {
                    (arguments.first(), arguments.get(1..))
                } else {
                    (arguments.get(1), arguments.get(2..))
                };
                let callback = callback.cloned().unwrap_or(Value::Undefined);
                if !is_callable(&callback) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "AsyncLocalStorage callback is not a function".into(),
                    });
                }
                let store = if exiting {
                    None
                } else {
                    Some(arguments.first().cloned().unwrap_or(Value::Undefined))
                };
                // Only *this* storage's binding is saved and put back. Restoring the whole
                // context would undo an `enterWith` the callback did on some *other* storage,
                // which node keeps: each storage's stack is its own, and one storage's `run`
                // has no opinion about another's.
                let displaced = self.held_store(&storage);
                self.async_context = bind_store(&self.async_context, &storage, store);
                let outcome = self.call_value(
                    &callback,
                    Value::Undefined,
                    rest.unwrap_or_default().to_vec(),
                    instruction,
                );
                // On both paths: a callback that throws must not leave its store in force for
                // whatever catches it.
                self.async_context = bind_store(&self.async_context, &storage, displaced);
                outcome
            }
            _ => unreachable!("every AsyncLocalStorage method is answered above"),
        }
    }

    /// What `getStore` answers: the held store, or `undefined` when there is none or the storage
    /// has been disabled.
    pub(super) fn current_store(&self, storage: &Rc<AsyncStorage>) -> Value {
        if storage.disabled.get() {
            return Value::Undefined;
        }
        self.held_store(storage).unwrap_or(Value::Undefined)
    }

    /// The binding itself, which is not the same question as `getStore`.
    ///
    /// A disabled storage still *has* whatever binding it had -- `disable` stops it being read,
    /// not being restored -- and `run` has to put back the binding it displaced rather than the
    /// answer a read would have given.
    pub(super) fn held_store(&self, storage: &Rc<AsyncStorage>) -> Option<Value> {
        self.async_context
            .iter()
            .find(|(held, _)| Rc::ptr_eq(held, storage))
            .map(|(_, store)| store.clone())
    }
}
