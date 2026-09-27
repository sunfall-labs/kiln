use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// `new Set(iterable)` or `new Map(entries)`.
    pub(super) fn construct_collection(
        &mut self,
        kind: Native,
        arguments: &[Value],
        current: usize,
    ) -> Result<Value, ExecutionError> {
        let is_map = matches!(kind, Native::MapConstructor | Native::WeakMapConstructor);
        let weak = matches!(
            kind,
            Native::WeakMapConstructor | Native::WeakSetConstructor
        );
        let mut data = ObjectData::default();
        data.set("@@entries", Value::Array(ArrayData::new(Vec::new())));
        if is_map {
            data.set("@@map", Value::Boolean(true));
        }
        if weak {
            data.set("@@weak", Value::Boolean(true));
        }
        // The link that makes `Object.getPrototypeOf(new Map())` answer `Map.prototype` rather
        // than `Object.prototype`. Method lookup does not go through it -- the reserved state
        // above is what answers a read, so the two cannot disagree -- but every question a
        // program asks *about* the chain does.
        data.set_prototype(Value::Native(
            Native::PrototypeOf(match (is_map, weak) {
                (true, false) => PrototypeTable::Map,
                (false, false) => PrototypeTable::Set,
                (true, true) => PrototypeTable::WeakMap,
                (false, true) => PrototypeTable::WeakSet,
            }),
            RealmId::PRIMARY,
        ));
        data.set(COLLECTION_SIZE, Value::Number(0.0));
        let collection = Value::Object(Rc::new(RefCell::new(data)));
        // A seeded collection goes through `add`/`set`, so deduplication and size accounting are
        // the same code the explicit calls use rather than a second copy of it.
        if let Some(source) = arguments.first().cloned()
            && !matches!(source, Value::Undefined | Value::Null)
        {
            // `AddEntriesFromIterable` steps the source and inserts what it got, one entry at a
            // time. Draining it first was not merely a different order: an entry whose getter
            // throws is how a program says "stop", and a source that never reports `done`
            // relies on it -- so `new Map(endless)` walked for ever accumulating values, and the
            // throw that would have ended it was never reached. It ran until the step limit.
            let method =
                self.read_property(&source, WellKnown::Iterator.property_key(), current)?;
            if !is_callable(&method) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!("{} is not iterable", source.to_display_string()),
                });
            }
            let iterator = self.call_value(&method, source.clone(), Vec::new(), current)?;
            let next = self.iterator_direct(&iterator, current)?;
            loop {
                let Some(item) = self.iterator_step(&iterator, &next, current)? else {
                    break;
                };
                // Anything that goes wrong from here is the iteration's failure, and the source
                // is told the walk is over before it travels. The close is discarded because the
                // original throw says more than a complaint from `return`.
                match self.insert_collection_entry(&collection, is_map, weak, item, current) {
                    Ok(()) => {}
                    Err(error) => {
                        self.close_discarding(&iterator, current);
                        return Err(error);
                    }
                }
            }
        }
        Ok(collection)
    }

    /// One entry of a collection being seeded from an iterable.
    ///
    /// A `Map` entry is a *pair*, read with two ordinary `Get`s, so an accessor in front of
    /// either half runs and may throw. A `Set` takes the item itself.
    pub(super) fn insert_collection_entry(
        &mut self,
        collection: &Value,
        is_map: bool,
        weak: bool,
        item: Value,
        current: usize,
    ) -> Result<(), ExecutionError> {
        let (key, value) = if is_map {
            // The specification requires an object here before it reads either half: a `Map`
            // seeded from `[1, 2]` refuses the number rather than reading `1[0]` as undefined.
            if !is_an_object(&item) && !is_callable(&item) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!("{} is not an entry pair", item.to_display_string()),
                });
            }
            (
                self.read_computed(&item, &Value::Number(0.0), current)?,
                self.read_computed(&item, &Value::Number(1.0), current)?,
            )
        } else {
            (item, Value::Undefined)
        };
        // A weak collection seeded from an iterable holds its entries to the same rule a later
        // `add` or `set` does: `new WeakSet([1])` is a TypeError, not a set holding a number no
        // reference could ever match. The insert below goes straight to the machine-free table,
        // so the check the dispatcher makes for the method forms is repeated here.
        if weak && !self.can_be_held_weakly(&key) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!(
                    "{} cannot be held weakly: a weak collection takes an object or an \
                     unregistered symbol",
                    key.to_display_string()
                ),
            });
        }
        // The collection's *own* insert, weak or strong: it is the one whose brand check this
        // receiver passes, and the two do the same work behind it.
        call_native(
            &self.intrinsics,
            match (is_map, weak) {
                (true, true) => Native::WeakMapSet,
                (true, false) => Native::MapSet,
                (false, true) => Native::WeakSetAdd,
                (false, false) => Native::SetAdd,
            },
            collection,
            &[key, value],
        )?;
        Ok(())
    }
}
