use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// `Date.prototype.getTime` or `setTime`, the two representation operations the shared
    /// `Date.prototype` (`kiln-builtins/src/date.ts`) captures at realm start.
    ///
    /// On the machine rather than in [`call_native`] because `setTime` converts its argument
    /// through a `valueOf` a program can write, and only the machine can run one.
    pub(super) fn date_dispatch(
        &mut self,
        method: DateMethod,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Date(instant) = receiver else {
            return Err(ExecutionError::TypeErrorMessage {
                message: "this is not a Date object.".to_owned(),
            });
        };
        match method {
            DateMethod::GetTime => Ok(Value::Number(instant.time.get())),
            DateMethod::SetTime => {
                // The argument is converted even though the result ignores the old instant --
                // `d.setTime()` is NaN rather than a no-op -- and after the receiver check,
                // which is the specification's order.
                let time = time_clip(self.as_number_argument(arguments.first(), instruction)?);
                instant.time.set(time);
                Ok(Value::Number(time))
            }
        }
    }
}
