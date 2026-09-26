use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// `new Promise(executor)`.
    ///
    /// Its own function because `class P extends Promise` reaches it through `super()`, and a
    /// second copy of the executor's error handling in that path is exactly the kind of drift
    /// that makes one form of a constructor behave differently from the other.
    pub(super) fn construct_promise(
        &mut self,
        arguments: &[Value],
        current: usize,
    ) -> Result<Value, ExecutionError> {
        let promise = PromiseState::pending();
        let executor = arguments.first().cloned().unwrap_or(Value::Undefined);
        // The executor runs *now*, synchronously, which is what lets it capture something and
        // settle the promise later.
        let outcome = self.call_catching(
            &executor,
            Value::Undefined,
            vec![
                Value::Settler(Rc::clone(&promise), false),
                Value::Settler(Rc::clone(&promise), true),
            ],
            current,
        )?;
        // An executor that throws rejects the promise, rather than failing the program:
        // `new Promise(() => { throw e })` is how a rejection is often written.
        if let Err(thrown) = outcome {
            self.settle(&promise, &Settled::Rejected(thrown));
        }
        Ok(Value::Promise(promise))
    }
}
