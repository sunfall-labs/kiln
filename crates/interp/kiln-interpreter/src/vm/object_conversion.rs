use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// One entry of `Object.fromEntries`, read the way the specification reads it.
    ///
    /// An entry that is not an object is a `TypeError`, not a row to skip:
    /// `Object.fromEntries([1])` answered `{}`, so a caller that had built its pairs wrongly got
    /// an empty object back and no indication of where.
    /// Both halves are ordinary `Get`s on
    /// whatever the iterator handed back, so an accessor at `0` or `1` runs and may throw.
    #[allow(clippy::wrong_self_convention)]
    pub(super) fn from_entries_pair(
        &mut self,
        entry: &Value,
        instruction: usize,
    ) -> Result<(Value, Value), ExecutionError> {
        if !is_an_object(entry) && !is_callable(entry) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!(
                    "iterator entry {} is not an object",
                    entry.to_display_string()
                ),
            });
        }
        let key = self.read_computed(entry, &Value::Number(0.0), instruction)?;
        let value = self.read_computed(entry, &Value::Number(1.0), instruction)?;
        Ok((key, value))
    }

    /// `ToPropertyKey` of a key that arrived from the program, as a value.
    ///
    /// Only an object needs converting: every primitive is already a key, and a Number left as
    /// one keeps the array fast path that recognises `a[0]` as an index. An object reduces
    /// through `Symbol.toPrimitive`, `valueOf` and `toString` -- the program's own code, which
    /// may return anything and may throw. The diagnostic-only `Value::property_key_lossy` could
    /// do none of it:
    /// it answered `"[object Object]"` for every object, so `o[{toString(){return "K"}}]` wrote
    /// under the wrong name and `o[{toString(){throw x}}]` did not throw at all.
    pub(super) fn computed_key(
        &mut self,
        key: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if !is_reflect_target(key) {
            return Ok(key.clone());
        }
        self.as_primitive(key, PrimitiveHint::String, instruction)
    }

    /// `ToPropertyKey` of an argument, which may run the program's own `toString`.
    pub(super) fn property_key_of_units(
        &mut self,
        key: Option<&Value>,
        instruction: usize,
    ) -> Result<StringUnits, ExecutionError> {
        let key = key.cloned().unwrap_or(Value::Undefined);
        // A symbol is a key already and must not be stringified; everything else reduces first,
        // and the reduction can be the program's code.
        if matches!(key, Value::Symbol(_)) {
            return key.property_key_units_checked();
        }
        let primitive = self.as_primitive(&key, PrimitiveHint::String, instruction)?;
        primitive.property_key_units_checked()
    }
}
