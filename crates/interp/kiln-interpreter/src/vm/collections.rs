use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// One of the seven set operations.
    ///
    /// Every one of them reads `size`, `has` and `keys` off its argument before touching the
    /// receiver, and then calls back into the program -- so the whole family runs here rather
    /// than in [`call_native`]. The branch on which side is smaller is the specification's own
    /// and it is observable: it decides whether the program's `has` runs or its `keys` iterator
    /// does, and a program can tell which.
    pub(super) fn set_operation(
        &mut self,
        operation: SetOperation,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let entries = require_set(receiver)?;
        let other = self.set_like(arguments.first(), instruction)?;
        match operation {
            SetOperation::Union => {
                let mut result = set_data_values(&entries);
                self.for_each_key(&other, instruction, |_, key, _| {
                    if !result.iter().any(|held| same_value_zero(held, &key)) {
                        result.push(key);
                    }
                    Ok(true)
                })?;
                Ok(new_set(result))
            }
            SetOperation::Intersection => {
                let mut result = Vec::new();
                if set_data_values(&entries).len() as f64 <= other.size {
                    // The receiver is walked afresh each step: `has` is the program's, and it
                    // can add to or delete from this very set while the walk is running.
                    let mut index = 0;
                    while index < entries.borrow().len() {
                        let held = entries.borrow().get(index).cloned();
                        index += 1;
                        let Some(held) = held else { break };
                        if matches!(held, Value::Native(Native::Tombstone, _)) {
                            continue;
                        }
                        if self.set_like_has(&other, &held, instruction)?
                            && !result.iter().any(|kept| same_value_zero(kept, &held))
                        {
                            result.push(held);
                        }
                    }
                } else {
                    self.for_each_key(&other, instruction, |_, key, _| {
                        if set_data_has(&entries, &key)
                            && !result.iter().any(|kept| same_value_zero(kept, &key))
                        {
                            result.push(key);
                        }
                        Ok(true)
                    })?;
                }
                Ok(new_set(result))
            }
            SetOperation::Difference => {
                let mut result = set_data_values(&entries);
                if result.len() as f64 <= other.size {
                    let mut kept = Vec::with_capacity(result.len());
                    for held in result {
                        if !self.set_like_has(&other, &held, instruction)? {
                            kept.push(held);
                        }
                    }
                    result = kept;
                } else {
                    self.for_each_key(&other, instruction, |_, key, _| {
                        result.retain(|held| !same_value_zero(held, &key));
                        Ok(true)
                    })?;
                }
                Ok(new_set(result))
            }
            SetOperation::SymmetricDifference => {
                let original = set_data_values(&entries);
                let mut result = original.clone();
                self.for_each_key(&other, instruction, |_, key, _| {
                    // Tested against what the receiver held when the walk began, not against
                    // the result: an element the argument yields twice must come back.
                    if original.iter().any(|held| same_value_zero(held, &key)) {
                        result.retain(|held| !same_value_zero(held, &key));
                    } else if !result.iter().any(|held| same_value_zero(held, &key)) {
                        result.push(key);
                    }
                    Ok(true)
                })?;
                Ok(new_set(result))
            }
            SetOperation::IsSubsetOf => {
                let held = set_data_values(&entries);
                // A set larger than the other cannot be inside it, and saying so costs no calls.
                if held.len() as f64 > other.size {
                    return Ok(Value::Boolean(false));
                }
                for value in held {
                    if !self.set_like_has(&other, &value, instruction)? {
                        return Ok(Value::Boolean(false));
                    }
                }
                Ok(Value::Boolean(true))
            }
            SetOperation::IsSupersetOf => {
                if (set_data_values(&entries).len() as f64) < other.size {
                    return Ok(Value::Boolean(false));
                }
                let mut answer = true;
                self.for_each_key(&other, instruction, |_, key, _| {
                    if set_data_has(&entries, &key) {
                        return Ok(true);
                    }
                    answer = false;
                    Ok(false)
                })?;
                Ok(Value::Boolean(answer))
            }
            SetOperation::IsDisjointFrom => {
                let held = set_data_values(&entries);
                if held.len() as f64 <= other.size {
                    for value in held {
                        if self.set_like_has(&other, &value, instruction)? {
                            return Ok(Value::Boolean(false));
                        }
                    }
                    return Ok(Value::Boolean(true));
                }
                let mut answer = true;
                self.for_each_key(&other, instruction, |_, key, _| {
                    if set_data_has(&entries, &key) {
                        answer = false;
                        return Ok(false);
                    }
                    Ok(true)
                })?;
                Ok(Value::Boolean(answer))
            }
        }
    }

    /// `GetSetRecord`: the three things one of the seven operations needs from its argument.
    ///
    /// Read in this order, and all three before anything else happens, because a program can
    /// watch the order with getters -- and because a `size` that is not a number has to be
    /// refused before a `has` is so much as looked for.
    pub(super) fn set_like(
        &mut self,
        other: Option<&Value>,
        instruction: usize,
    ) -> Result<SetLike, ExecutionError> {
        let object = match other {
            Some(value) if is_reflect_target(value) => value.clone(),
            _ => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "the argument is not an object".to_owned(),
                });
            }
        };
        let raw = self.read_property(&object, "size", instruction)?;
        // An absent `size` reaches here as `undefined` and reduces to NaN, which is the case
        // this refuses: a set-like with no size is not one.
        let size = self.as_number_argument(Some(&raw), instruction)?;
        if size.is_nan() {
            return Err(ExecutionError::TypeErrorMessage {
                message: "the argument has no usable size".to_owned(),
            });
        }
        let size = size.trunc();
        if size < 0.0 {
            return Err(ExecutionError::SetSizeOutOfRange);
        }
        let has = self.read_property(&object, "has", instruction)?;
        if !is_callable(&has) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "the argument's has is not a function".to_owned(),
            });
        }
        let keys = self.read_property(&object, "keys", instruction)?;
        if !is_callable(&keys) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "the argument's keys is not a function".to_owned(),
            });
        }
        Ok(SetLike {
            object,
            size,
            has,
            keys,
        })
    }

    /// `other.has(value)`, as the specification's `Call(setRec.[[Has]], setRec.[[SetObject]])`.
    pub(super) fn set_like_has(
        &mut self,
        other: &SetLike,
        value: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        let answered = self.call_value(
            &other.has,
            other.object.clone(),
            Vec::from([value.clone()]),
            instruction,
        )?;
        Ok(answered.is_truthy())
    }

    /// Walk a set-like's `keys`, stopping early when the visitor says so.
    ///
    /// Stopping is a close, not an abandonment: `isSupersetOf` that finds a missing element has
    /// to give the argument's iterator its `return` before answering false.
    pub(super) fn for_each_key(
        &mut self,
        other: &SetLike,
        instruction: usize,
        mut visit: impl FnMut(&mut Self, Value, usize) -> Result<bool, ExecutionError>,
    ) -> Result<(), ExecutionError> {
        let iterator =
            self.call_value(&other.keys, other.object.clone(), Vec::new(), instruction)?;
        let next = self.iterator_direct(&iterator, instruction)?;
        loop {
            let Some(value) = self.iterator_step(&iterator, &next, instruction)? else {
                return Ok(());
            };
            match visit(self, canonical_key(value), instruction) {
                Ok(true) => {}
                Ok(false) => {
                    self.close_iterator(&iterator, instruction)?;
                    return Ok(());
                }
                Err(error) => {
                    self.close_discarding(&iterator, instruction);
                    return Err(error);
                }
            }
        }
    }
}
