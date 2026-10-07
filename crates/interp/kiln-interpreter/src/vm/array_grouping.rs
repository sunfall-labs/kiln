use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// `Object.groupBy` and `Map.groupBy`.
    ///
    /// One walk with two shapes of result, and the difference is what a key is allowed to be: a
    /// `Map`'s keys are values compared by SameValueZero, an object's are property keys, so the
    /// object form runs `ToPropertyKey` on whatever the callback answered and the Map form does
    /// not. The object it builds has a *null* prototype, so a group called `toString` is a group
    /// rather than a collision with `Object.prototype`.
    pub(super) fn group_by(
        &mut self,
        into_map: bool,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let items = arguments.first().cloned().unwrap_or(Value::Undefined);
        let callback = arguments.get(1).cloned().unwrap_or(Value::Undefined);
        if !is_callable(&callback) {
            return Err(ExecutionError::NotCallable {
                instruction,
                value: callback.type_name(),
            });
        }
        let walked = self.drain_iterable(&items, instruction)?;
        // Insertion order is the order the first member of each group arrived in, which both
        // results have to preserve -- a `Map` by its own contract, an object because integer-like
        // keys come first and the rest follow in creation order.
        let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
        for (index, item) in walked.into_iter().enumerate() {
            let produced = self.call_value(
                &callback,
                Value::Undefined,
                Vec::from([item.clone(), Value::Number(index as f64)]),
                instruction,
            )?;
            let key = if into_map {
                canonical_key(produced)
            } else {
                Value::String(self.property_key_of_units(Some(&produced), instruction)?)
            };
            match groups
                .iter_mut()
                .find(|(held, _)| same_value_zero(held, &key))
            {
                Some((_, members)) => members.push(item),
                None => groups.push((key, Vec::from([item]))),
            }
        }
        if into_map {
            let entries: Vec<Value> = groups
                .into_iter()
                .map(|(key, members)| {
                    Value::Array(ArrayData::new(Vec::from([
                        key,
                        Value::Array(ArrayData::new(members)),
                    ])))
                })
                .collect();
            let size = entries.len() as f64;
            let mut data = ObjectData::default();
            data.set("@@entries", Value::Array(ArrayData::new(entries)));
            data.set("@@map", Value::Boolean(true));
            data.set_prototype(Value::Native(
                Native::PrototypeOf(PrototypeTable::Map),
                RealmId::PRIMARY,
            ));
            data.set(COLLECTION_SIZE, Value::Number(size));
            return Ok(Value::Object(tracked_object(data)));
        }
        let mut result = ObjectData::default();
        result.set_prototype(Value::Null);
        for (key, members) in groups {
            let key = key.property_key_units_checked()?;
            result.set_units(&key, Value::Array(ArrayData::new(members)));
        }
        Ok(Value::Object(tracked_object(result)))
    }
}
