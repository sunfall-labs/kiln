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
        if weak {
            data.weak_collection_entries = Some(Vec::new());
        } else {
            data.set("@@entries", Value::Array(ArrayData::new(Vec::new())));
        }
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
        if weak {
            if let Value::Object(object) = &collection {
                let module_map = self.gc_module_map();
                let mut module_map = module_map.borrow_mut();
                if module_map.gc_weak_collections.len()
                    >= module_map.gc_weak_collections_prune_at.max(1024)
                {
                    module_map
                        .gc_weak_collections
                        .retain(|entry| entry.strong_count() > 0);
                    module_map.gc_weak_collections_prune_at = module_map
                        .gc_weak_collections
                        .len()
                        .saturating_add(module_map.gc_weak_collections.len().max(1024));
                }
                module_map.gc_weak_collections.push(Rc::downgrade(object));
            }
        }
        // A seeded collection goes through `add`/`set`, so deduplication and size accounting are
        // the same code the explicit calls use rather than a second copy of it.
        if let Some(source) = arguments.first().cloned()
            && !matches!(source, Value::Undefined | Value::Null)
        {
            // Get the adder once, before opening the iterator. An override may throw or
            // mutate the prototype; every entry must still call this captured method.
            let adder =
                self.read_property(&collection, if is_map { "set" } else { "add" }, current)?;
            if !is_callable(&adder) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "collection adder is not callable".to_owned(),
                });
            }
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
                match self.insert_collection_entry(&collection, &adder, is_map, item, current) {
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
        adder: &Value,
        is_map: bool,
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
        let arguments = if is_map { vec![key, value] } else { vec![key] };
        self.call_value(adder, collection.clone(), arguments, current)?;
        Ok(())
    }
}
