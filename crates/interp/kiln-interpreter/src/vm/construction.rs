use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// The target of a `Reflect` operation, refused if it is not an object.
    ///
    /// The refusal is the difference between these and their `Object` namesakes: `Object.keys(1)`
    /// coerces and answers, where `Reflect.ownKeys(1)` is a `TypeError`. A `Reflect` operation is
    /// the object protocol spelled out, and a primitive is not on it.
    pub(super) fn reflect_target(
        &self,
        target: Option<&Value>,
        operation: &str,
    ) -> Result<Value, ExecutionError> {
        match target {
            Some(value) if is_reflect_target(value) => Ok(value.clone()),
            _ => Err(ExecutionError::TypeErrorMessage {
                message: format!("Reflect.{operation} called on non-object"),
            }),
        }
    }

    /// `CreateListFromArrayLike`: the arguments `Reflect.apply` and `Reflect.construct` forward.
    ///
    /// Reads `length` and then each index, so a program's getters run in the order the
    /// specification gives -- and a value that is not an object at all is a `TypeError` rather
    /// than an empty call, which is what the previous "an array or nothing" reading made it.
    pub(super) fn argument_list(
        &mut self,
        list: Option<&Value>,
        instruction: usize,
    ) -> Result<Vec<Value>, ExecutionError> {
        let list = match list {
            Some(value) if is_reflect_target(value) => value.clone(),
            _ => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "arguments list is not an object".to_owned(),
                });
            }
        };
        if let Value::Array(items) = &list
            && array_walks_densely(items)
            && items.length.get() == items.borrow().len()
        {
            return Ok(dense_items(&items.borrow()));
        }
        let length = self
            .array_like_length(&list, instruction)?
            .min(u64::from(u32::MAX)) as usize;
        let mut collected = Vec::with_capacity(length.min(1024));
        for index in 0..length {
            collected.push(self.read_property(&list, &index.to_string(), instruction)?);
        }
        Ok(collected)
    }

    /// `Construct(target, args, newTarget)`, as a value rather than as a push onto the stack.
    ///
    /// [`Self::construct`] leaves a compiled constructor's frame for the interpreter loop to
    /// drive, which is what the `New` instruction wants and what a built-in calling it cannot
    /// use. This drives that frame to completion instead.
    pub(super) fn construct_value(
        &mut self,
        target: Value,
        arguments: Vec<Value>,
        new_target: Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let depth = self.frames.len();
        self.construct_as(target, arguments, new_target, instruction)?;
        if self.frames.len() > depth {
            return Ok(self.run_frames(depth)?.value);
        }
        self.pop(instruction)
    }
}
