use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// `LengthOfArrayLike(O)`: `ToLength(Get(O, "length"))`.
    ///
    /// The conversion is the program's own -- `{length: {valueOf() { throw x }}}` throws here --
    /// and a `length` that is an object with no `valueOf` reads as `NaN`, which is an empty walk
    /// rather than an error.
    pub(super) fn array_like_length(
        &mut self,
        target: &Value,
        instruction: usize,
    ) -> Result<u64, ExecutionError> {
        let raw = self.read_property(target, "length", instruction)?;
        let number = self.as_number_argument(Some(&raw), instruction)?;
        Ok(Self::to_length(number))
    }

    /// The element an array holds at `index` in its dense storage, when that storage is the
    /// whole answer.
    ///
    /// An array reaches the generic walk when it is sparse, when an element has been defined
    /// with flags of its own, or when its `length` is locked -- and in each of those cases only
    /// *some* positions are unusual. This is the ordinary one, answered without building the
    /// index's spelling or walking anything: the walk is the specification's, and this is the
    /// step of it that an array can answer directly.
    pub(super) fn dense_element(target: &Value, index: u64) -> Option<Value> {
        let Value::Array(items) = target else {
            return None;
        };
        if !items.elements.borrow().is_empty() {
            return None;
        }
        #[expect(clippy::cast_possible_truncation)]
        let index = index as usize;
        items
            .borrow()
            .get(index)
            .filter(|value| !Self::is_hole(value))
            .cloned()
    }

    /// `HasProperty(O, ToString(index))`, prototype chain and proxy trap included.
    pub(super) fn element_has(
        &mut self,
        target: &Value,
        index: u64,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        if Self::dense_element(target, index).is_some() {
            return Ok(true);
        }
        // A hole on an ordinary array has no inherited answer once both indexed prototype
        // surfaces are known to be empty. Check the invariant for this position each time, so a
        // callback can still add an element later in the walk; custom chains and exotic element
        // descriptors keep the generic `HasProperty` path. Array indices stop below 2^32-1.
        if let Value::Array(items) = target
            && index < u64::from(u32::MAX)
            && items.elements.borrow().is_empty()
            && items.properties.borrow().prototype().is_none()
            && !self.intrinsics.indexed_array_prototype.get()
        {
            return Ok(false);
        }
        self.has(target, &Self::element_key(index), instruction)
    }

    /// `Get(O, ToString(index))`, which may run a getter or a proxy trap.
    pub(super) fn element_get(
        &mut self,
        target: &Value,
        index: u64,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if let Some(value) = Self::dense_element(target, index) {
            return Ok(value);
        }
        self.read_computed(target, &Self::element_key(index), instruction)
    }

    /// `HasProperty(O, k)` and `Get(O, k)` in one step, for the walks that ask both: the value
    /// when the position is there, and `None` when it is not.
    ///
    /// Not a shortcut past the specification's two steps -- a receiver that can observe them
    /// still gets both -- but a position an array holds in its dense storage answers both at
    /// once, and a walk that asked twice paid for the borrow and the clone twice.
    pub(super) fn element_read(
        &mut self,
        target: &Value,
        index: u64,
        instruction: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        if let Some(value) = Self::dense_element(target, index) {
            return Ok(Some(value));
        }
        if !self.element_has(target, index, instruction)? {
            return Ok(None);
        }
        Ok(Some(self.element_get(target, index, instruction)?))
    }

    /// `Set(O, ToString(index), value, true)`.
    pub(super) fn element_set(
        &mut self,
        target: &Value,
        index: u64,
        value: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        // An element that is already there, on an array with nothing unusual recorded about its
        // elements and nothing frozen, is writable -- so the write is the write, with no
        // descriptor to consult and no key to spell.
        if let Value::Array(items) = target
            && items.elements.borrow().is_empty()
            && !items.properties.borrow().is_frozen()
        {
            #[expect(clippy::cast_possible_truncation)]
            let at = index as usize;
            let mut elements = items.borrow_mut();
            if at < elements.len() {
                elements[at] = value;
                return Ok(());
            }
        }
        self.set_or_throw(target, &Self::element_key(index), value, instruction)
    }

    /// `DeletePropertyOrThrow(O, ToString(index))`.
    pub(super) fn element_delete(
        &mut self,
        target: &Value,
        index: u64,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let key = Self::element_key(index);
        if self.delete(target, &key, instruction)? {
            return Ok(());
        }
        Err(ExecutionError::TypeErrorMessage {
            message: format!("Cannot delete property '{index}' of an object"),
        })
    }

    /// `Set(O, "length", length, true)`.
    pub(super) fn set_array_like_length(
        &mut self,
        target: &Value,
        length: u64,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        #[expect(clippy::cast_precision_loss)]
        let length = length as f64;
        self.set_or_throw(
            target,
            &Value::String("length".to_owned().into()),
            Value::Number(length),
            instruction,
        )
    }

    /// `Set(O, P, V, true)`: the throwing spelling of a write.
    ///
    /// The ordinary assignment path cannot raise -- the machine does not know whether the code
    /// that wrote it was strict -- but every write inside an `Array.prototype` method passes
    /// `true` for `Throw`, so a `push` onto a frozen object is a `TypeError` rather than a
    /// silence that reads as success.
    pub(super) fn set_or_throw(
        &mut self,
        target: &Value,
        key: &Value,
        value: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        if let Value::Proxy(proxy) = target {
            let proxy = Rc::clone(proxy);
            let inner = proxy.target.clone();
            return match self.proxy_trap(&proxy, "set", instruction)? {
                Some(trap) => {
                    let handler = proxy.handler.clone();
                    let arguments = vec![
                        inner.clone(),
                        self.trap_key(key),
                        value.clone(),
                        target.clone(),
                    ];
                    let answered = self.call_value(&trap, handler, arguments, instruction)?;
                    if !answered.is_truthy() {
                        return Err(Self::write_refused(key));
                    }
                    self.check_set_invariant(&inner, key, &value, instruction)
                }
                None => self.set_or_throw(&inner, key, value, instruction),
            };
        }
        // Ordinary numeric writes need no key spelling or prototype scan when the array
        // has default descriptors and no built-in prototype can supply an indexed property.
        // Keep the shared indexed-write path (and its conservative sparse flag) authoritative.
        if let (Value::Array(items), Value::Number(index)) = (target, key)
            && *index >= 0.0
            && *index < f64::from(u32::MAX)
            && index.fract() == 0.0
            && items.length_writable.get()
            && items.elements.borrow().is_empty()
            && items.properties.borrow().is_extensible()
            && !items.properties.borrow().is_frozen()
            && items.properties.borrow().prototype().is_none()
            && !self.intrinsics.indexed_array_prototype.get()
        {
            return self.set_array_index(target, items, *index as usize, value, instruction);
        }
        let units = key.property_key_units_checked()?;
        if !self.write_accepted_key(target, PropertyKeyRef::Units(&units)) {
            return Err(Self::write_refused(key));
        }
        self.set_computed(target, key, value, instruction)
    }

    /// Whether an ordinary `[[Set]]` of `name` would take, which is what decides whether the
    /// throwing spelling throws.
    ///
    /// `OrdinarySetWithOwnDescriptor` does not stop at the own property. When there is none the
    /// chain answers instead, and both of the ways it can refuse are inherited: a non-writable
    /// data property on a prototype forbids shadowing it, and a getter-only accessor there
    /// swallows the write rather than creating anything. Only when the chain has nothing to say
    /// does it come down to whether the receiver will take a new property at all.
    pub(super) fn write_accepted_key(&self, target: &Value, key: PropertyKeyRef<'_>) -> bool {
        if refuses_every_write(target) {
            return false;
        }
        if let Value::Function(closure) = target
            && key.matches_ascii("prototype")
        {
            return !closure.properties.borrow().is_frozen();
        }
        // `__proto__` is an accessor on `Object.prototype` with a setter, and the setter is what
        // decides -- it refuses a cycle and a non-extensible target by throwing, in sloppy mode
        // as well as strict. So the write is always "accepted" here and `set_computed` answers.
        if key.matches_ascii("__proto__") && is_reflect_target(target) {
            return true;
        }
        match own_property_state_key(&self.intrinsics, target, key) {
            Some((Property::Value(_), attributes)) => return attributes.writable,
            // A setter-less accessor discards the write, which the throwing spelling reports.
            Some((Property::Accessor { set, .. }, _)) => return set.is_some(),
            None => {}
        }
        // Use the same descriptor authority for ordinary and intrinsic prototypes. The
        // latter may have materialized properties that replace (or add to) the static table.
        let mut current = prototype_of(target);
        while matches!(
            current,
            Value::Object(_)
                | Value::Array(_)
                | Value::Function(_)
                | Value::Bound(_)
                | Value::Native(_, _)
        ) {
            match own_property_state_key(&self.intrinsics, &current, key) {
                // A non-writable data property on a prototype cannot be shadowed by an
                // assignment: `Object.create(Object.freeze({x: 1})).x = 2` is refused.
                Some((Property::Value(_), attributes)) if !attributes.writable => return false,
                // A writable one is shadowed, which the receiver has to have room for.
                Some((Property::Value(_), _)) => break,
                Some((Property::Accessor { set, .. }, _)) => return set.is_some(),
                None => {}
            }
            current = prototype_of(&current);
        }
        if let Value::Array(items) = target
            && let Some(index) = array_index_key(key)
        {
            return Self::array_index_creation_allowed(items, index);
        }
        is_extensible_target(&self.intrinsics, target)
    }

    pub(super) fn write_refused(key: &Value) -> ExecutionError {
        ExecutionError::TypeErrorMessage {
            message: format!(
                "Cannot assign to read only property '{}' of object",
                key.property_key_lossy()
            ),
        }
    }

    /// `ToObject(this value)` for the receiver of a generic array method.
    ///
    /// Step one of every one of them, and the reason a redirected call onto a primitive answers
    /// at all: `Array.prototype.reduce.call("abc", f, 1)` hands the callback a `String` wrapper
    /// -- which the corpus checks by tag -- and walks its characters as indexed properties.
    /// `null` and `undefined` have no object to become and stop here.
    pub(super) fn as_receiver_object(&mut self, receiver: Value) -> Result<Value, ExecutionError> {
        match receiver {
            Value::Undefined | Value::Null => Err(ExecutionError::TypeErrorMessage {
                message: format!(
                    "Array.prototype method called on {}",
                    receiver.to_display_string()
                ),
            }),
            primitive @ (Value::String(_)
            | Value::Number(_)
            | Value::Boolean(_)
            | Value::Symbol(_)
            | Value::BigInt(_)) => Ok(wrap_primitive(primitive)),
            // Everything else is already an object here: a date, a regexp, a promise and a
            // built-in namespace are all value kinds of their own rather than boxed primitives,
            // and boxing one would hide the properties a program put on it.
            other => Ok(other),
        }
    }

    /// Whether this callee has a generic implementation that walks its receiver live.
    pub(super) fn walks_a_live_receiver(callee: &Value) -> bool {
        match callee {
            Value::Native(native, _) => native.walks_a_live_receiver(),
            Value::Callback(method, _) => matches!(
                method,
                CallbackMethod::Map
                    | CallbackMethod::ParallelMap
                    | CallbackMethod::ForEach
                    | CallbackMethod::Filter
                    | CallbackMethod::Reduce
                    | CallbackMethod::ReduceRight
                    | CallbackMethod::Sort
                    | CallbackMethod::Find
                    | CallbackMethod::FindIndex
                    | CallbackMethod::FindLast
                    | CallbackMethod::FindLastIndex
                    | CallbackMethod::Some
                    | CallbackMethod::Every
                    | CallbackMethod::FlatMap
            ),
            _ => false,
        }
    }

    /// `ArraySpeciesCreate(originalArray, length)`: what a derived array method builds into.
    ///
    /// Not an implementation detail. `map`, `filter`, `slice`, `splice`, `concat`, `flat` and
    /// `flatMap` each produce a *new* array, and the language lets the receiver say what kind:
    /// a subclass of Array gets its subclass back, and a program that sets `constructor` or
    /// `Symbol.species` gets whatever it named. Building a plain array regardless is the answer
    /// only when the receiver is not an array at all, or has said nothing.
    ///
    /// Every step here is observable -- reading `constructor`, then `@@species` off it, then
    /// constructing -- which is why this cannot live in the pure table and why the seven methods
    /// take the machine's walk whether or not their receiver is a dense array. One of them
    /// consulting the species while another quietly did not would be worse than none doing it.
    pub(super) fn array_species_create(
        &mut self,
        original: &Value,
        length: u64,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if !matches!(unwrap_proxy(original), Value::Array(_)) {
            return Self::create_array_of_length(length);
        }
        let constructor = self.read_property(original, "constructor", instruction)?;
        // A constructor that is an object is asked for its species; `null` there means "plain
        // array" rather than an error, which is what lets a program opt out by writing it.
        let species = if is_an_object(&constructor) {
            let named = self.read_computed(
                &constructor,
                &Value::Symbol(Symbol::WellKnown(WellKnown::Species)),
                instruction,
            )?;
            if matches!(named, Value::Null) {
                Value::Undefined
            } else {
                named
            }
        } else {
            constructor
        };
        if matches!(species, Value::Undefined) {
            return Self::create_array_of_length(length);
        }
        if !is_constructor(&species) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{} is not a constructor", species.to_display_string()),
            });
        }
        // The plain `Array` constructor is the common case and builds exactly what
        // `ArrayCreate` would, so it is answered without a call.
        if matches!(species, Value::Native(Native::ArrayConstructor, _)) {
            return Self::create_array_of_length(length);
        }
        #[expect(clippy::cast_precision_loss)]
        let requested = Value::Number(length as f64);
        self.construct_value(species.clone(), vec![requested], species, instruction)
    }

    /// `ArrayCreate(length)`, which is what `ArraySpeciesCreate` reduces to when the receiver is
    /// not an array: the species lookup is skipped entirely for one that is not.
    pub(super) fn create_array_of_length(length: u64) -> Result<Value, ExecutionError> {
        #[expect(clippy::cast_precision_loss)]
        let requested = length as f64;
        if requested > MAX_ARRAY_LENGTH {
            return Err(ExecutionError::InvalidArrayLength);
        }
        if length > MAX_DENSE_ARRAY as u64 {
            return Err(ExecutionError::ArrayTooLarge {
                length: MAX_DENSE_ARRAY,
            });
        }
        #[expect(clippy::cast_possible_truncation)]
        let length = length as usize;
        Ok(Value::Array(ArrayData::new(vec![
            Value::Native(
                Native::Hole,
                RealmId::PRIMARY
            );
            length
        ])))
    }

    /// `CreateDataPropertyOrThrow(A, ToString(index), value)` on an array this method made.
    pub(super) fn create_element(
        &mut self,
        array: &Value,
        index: u64,
        value: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let Value::Array(items) = array else {
            // Whatever the species built. `CreateDataPropertyOrThrow` rather than an assignment:
            // a definition bypasses an inherited setter, which is what "create" means, and it
            // raises rather than dropping the element when the object refuses.
            return self.define_element(array, index, value, instruction);
        };
        // An array the species handed back may already have something recorded at this position
        // -- `Object.defineProperty(q, 0, {writable: false})` on a fresh one is exactly what the
        // corpus checks -- and a *definition* replaces it, flags and all, where the dense write
        // below would leave the old flags standing over a new value.
        #[expect(clippy::cast_possible_truncation)]
        let at = index as usize;
        if array_exotic_element(items, at).is_some()
            || items.properties.borrow().is_frozen()
            || !items.properties.borrow().is_extensible()
        {
            return self.define_element(array, index, value, instruction);
        }
        if index >= MAX_DENSE_ARRAY as u64 {
            return Err(ExecutionError::ArrayTooLarge {
                length: MAX_DENSE_ARRAY,
            });
        }
        #[expect(clippy::cast_possible_truncation)]
        let index = index as usize;
        let mut elements = items.borrow_mut();
        if index >= elements.len() {
            items.sparse.set(true);
            elements.resize(index + 1, Value::Native(Native::Hole, RealmId::PRIMARY));
        }
        elements[index] = value;
        Ok(())
    }

    /// Shorten or lengthen an array this method made, so that its `length` is what the walk
    /// counted rather than what it happened to reach.
    /// `CreateDataPropertyOrThrow(O, ToString(index), value)` on something that is not an
    /// array: whatever `ArraySpeciesCreate` built when the receiver named a species.
    pub(super) fn define_element(
        &mut self,
        target: &Value,
        index: u64,
        value: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let key = index.to_string();
        // Integer-indexed typed arrays implement [[DefineOwnProperty]] by writing their
        // element, rather than by adding a normal property. An out-of-bounds index is a
        // refusal (CreateDataPropertyOrThrow turns that into a TypeError), while conversion
        // still precedes the bounds check as it does for every typed-array write.
        if let Value::TypedArray(view) = target {
            let _ = coerce_element(view.kind, &value)?;
            #[expect(clippy::cast_possible_truncation)]
            let at = index as usize;
            if at >= view.len() {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!("Cannot define property '{key}' of object"),
                });
            }
            view.set(at, &value)?;
            return Ok(());
        }
        // Use the public DefineProperty operation so a species constructor returning a Proxy
        // observes its defineProperty trap and its target invariants. Calling the storage helper
        // directly would silently drop the definition because Proxy has no local property store.
        let mut data = ObjectData::default();
        data.set("value", value);
        data.set("writable", Value::Boolean(true));
        data.set("enumerable", Value::Boolean(true));
        data.set("configurable", Value::Boolean(true));
        self.dispatch_native(
            Native::ObjectDefineProperty,
            &Value::Undefined,
            &[
                target.clone(),
                Value::String(key.into()),
                Value::Object(Rc::new(RefCell::new(data))),
            ],
            instruction,
        )?;
        Ok(())
    }

    /// Set the finished length of what a method built, whatever kind of object that is.
    pub(super) fn finish_created_array(
        &mut self,
        array: &Value,
        length: u64,
    ) -> Result<(), ExecutionError> {
        if matches!(array, Value::Array(_)) {
            Self::truncate_created_array(array, length);
        }
        Ok(())
    }

    pub(super) fn truncate_created_array(array: &Value, length: u64) {
        let Value::Array(items) = array else {
            return;
        };
        #[expect(clippy::cast_possible_truncation)]
        let length = length as usize;
        let mut elements = items.borrow_mut();
        if length <= elements.len() {
            elements.truncate(length);
        } else {
            items.sparse.set(true);
            elements.resize(length, Value::Native(Native::Hole, RealmId::PRIMARY));
        }
    }

    /// `IsConcatSpreadable(O)`: `@@isConcatSpreadable` when it is defined, and otherwise whether
    /// the value is an array.
    pub(super) fn is_concat_spreadable(
        &mut self,
        value: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        if !is_reflect_target(value) {
            return Ok(false);
        }
        let marker = self.read_property(
            value,
            WellKnown::IsConcatSpreadable.property_key(),
            instruction,
        )?;
        if !matches!(marker, Value::Undefined) {
            return Ok(marker.is_truthy());
        }
        Ok(matches!(unwrap_proxy(value), Value::Array(_)))
    }
}

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// One of the four copying array methods.
    ///
    /// The receiver is read once, into a dense list, before anything else happens. That is what
    /// makes these safe where their mutating counterparts are not: a comparator or a `valueOf`
    /// that reaches back and changes the receiver cannot change the answer, because the answer
    /// was already taken from the receiver the specification saw.
    ///
    /// Every hole becomes `undefined`, which is the other difference from the mutating pair: the
    /// result is always a dense array, however sparse the receiver was.
    pub(super) fn array_copy(
        &mut self,
        method: ArrayCopy,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Some(items) = dense_array(receiver) else {
            return self.generic_array_copy(method, receiver, arguments, instruction);
        };
        let source = dense_items(&items.borrow());
        let length = source.len();
        match method {
            ArrayCopy::With => {
                let relative = self.as_number_argument(arguments.first(), instruction)?;
                let index = relative_position(relative, length);
                // An index outside the array is a `RangeError`, not an appended element: `with`
                // answers a copy of the *same* length, and there is nowhere to put it.
                let Some(index) = index.filter(|index| *index < length) else {
                    return Err(ExecutionError::ArrayIndexOutOfRange);
                };
                let mut copied = source;
                // The value is stored as it was handed over: this is the one argument of the
                // four that must not be converted to anything.
                copied[index] = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                Ok(Value::Array(ArrayData::new(copied)))
            }
            ArrayCopy::ToReversed => {
                let mut copied = source;
                copied.reverse();
                Ok(Value::Array(ArrayData::new(copied)))
            }
            ArrayCopy::ToSorted => {
                // Sorted through the same merge the in-place `sort` uses, on a copy: one
                // ordering, so the two can never disagree about where a comparator's ties land.
                let copy = Value::Array(ArrayData::new(source));
                self.array_sort(&copy, arguments, instruction)
            }
            ArrayCopy::ToSpliced => {
                let start = self.as_number_argument(arguments.first(), instruction)?;
                let start = relative_position(start, length).unwrap_or(0).min(length);
                // With no second argument nothing is removed; with one, it is clamped to what is
                // actually there. `splice(1)` removing the tail is the *one-argument* form, and
                // `toSpliced` keeps that distinction.
                let removed = match arguments.len() {
                    // No argument at all removes nothing; a `start` with no count removes
                    // everything from there on. The two are different calls, and `toSpliced()`
                    // answering a copy where `toSpliced(1)` answers a truncation is the
                    // distinction the specification draws.
                    0 => 0,
                    1 => length - start,
                    _ => {
                        let count = self.as_number_argument(arguments.get(1), instruction)?;
                        if count.is_nan() || count <= 0.0 {
                            0
                        } else {
                            (count.min(length as f64) as usize).min(length - start)
                        }
                    }
                };
                let mut copied = Vec::with_capacity(length - removed + arguments.len());
                copied.extend_from_slice(&source[..start]);
                copied.extend(arguments.iter().skip(2).cloned());
                copied.extend_from_slice(&source[start + removed..]);
                Ok(Value::Array(ArrayData::new(copied)))
            }
        }
    }
}
