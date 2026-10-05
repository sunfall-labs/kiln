use super::*;

/// How far a prototype chain is followed before a read gives up.
///
/// A chain this long is a program that built a cycle -- `setPrototypeOf` refuses to make one, but
/// nothing stops an object graph from being deep -- and the alternative to a bound is exhausting
/// the Rust stack, which aborts the process instead of reporting anything.
pub(super) const MAX_PROTOTYPE_DEPTH: usize = 10_000;

/// How far `<<` may widen a BigInt, in bits.
///
/// The language puts no bound on the type -- an implementation is expected to run out of memory
/// eventually -- but "out of memory" is not an outcome a program can catch, and `1n << (2n ** 64n)`
/// is a shift count no allocation could ever satisfy. Every engine answers it with a `RangeError`
/// rather than an attempt, so this is where that answer is decided: sixteen million bits is two
/// megabytes, which is far past any use and far short of a machine.
pub(super) const MAX_BIGINT_SHIFT: u64 = 1 << 24;

impl ObjectData {
    /// An own property slot, ignoring the prototype chain.
    pub fn get_own_property(&self, key: &str) -> Option<&Property> {
        self.get_own_property_key(PropertyKeyRef::Utf8(key))
    }

    /// Exact-unit counterpart of [`ObjectData::get_own_property`].
    pub fn get_own_property_units(&self, key: &StringUnits) -> Option<&Property> {
        self.get_own_property_key(PropertyKeyRef::Units(key))
    }

    pub(super) fn get_own_property_key(&self, key: PropertyKeyRef<'_>) -> Option<&Property> {
        self.properties
            .iter()
            .find(|slot| key.matches_slot(slot))
            .map(|slot| &slot.property)
    }

    pub fn get_own_attributes_units(&self, key: &StringUnits) -> Option<Attributes> {
        self.get_own_attributes_key(PropertyKeyRef::Units(key))
    }

    /// The three flags an own property carries, or `None` when there is no such property.
    pub fn get_own_attributes(&self, key: &str) -> Option<Attributes> {
        self.get_own_attributes_key(PropertyKeyRef::Utf8(key))
    }

    pub(super) fn get_own_attributes_key(&self, key: PropertyKeyRef<'_>) -> Option<Attributes> {
        self.properties
            .iter()
            .find(|slot| key.matches_slot(slot))
            .map(|slot| slot.attributes)
    }

    /// An own *data* property, ignoring the prototype chain and any accessor.
    ///
    /// For the VM's own slots, which are always data and never reached by program syntax. A
    /// program-visible read has to go through the interpreter, because an accessor is a call.
    pub fn get_own(&self, key: &str) -> Option<&Value> {
        match self.get_own_property(key) {
            Some(Property::Value(value)) => Some(value),
            _ => None,
        }
    }

    /// An owned own data value, resolving a namespace export through its live cell.
    ///
    /// Host adapters that intentionally inspect data properties without invoking accessors use
    /// this instead of `get_own`, whose borrowed result can only expose the stored slot.
    pub fn get_own_value(&self, key: &str) -> Option<Value> {
        self.get_own_value_key(PropertyKeyRef::Utf8(key))
    }

    /// Exact-unit counterpart of [`ObjectData::get_own_value`].
    pub fn get_own_value_units(&self, key: &StringUnits) -> Option<Value> {
        self.get_own_value_key(PropertyKeyRef::Units(key))
    }

    fn get_own_value_key(&self, key: PropertyKeyRef<'_>) -> Option<Value> {
        self.properties
            .iter()
            .enumerate()
            .find(|(_, slot)| key.matches_slot(slot))
            .and_then(|(index, slot)| match self.property_at_slot(index, slot) {
                Property::Value(value) => Some(value),
                Property::Accessor { .. } => None,
            })
    }

    /// The primitive this object boxes, or `None` for an ordinary object.
    pub fn primitive(&self) -> Option<&Value> {
        self.primitive.as_ref()
    }

    /// The built-in method table this object's prototype chain ends at, if any.
    ///
    /// A class that extends a built-in gets that built-in's prototype *marker* at the end of its
    /// chain, and [`ObjectData::lookup`] stops there because a marker is not an object with
    /// properties to read. This carries the read its last step, which is the whole of what makes
    /// `class T extends Iterator {}` give its instances `toArray`.
    pub fn prototype_table(&self) -> Option<(PrototypeTable, Option<RealmId>)> {
        let mut current = self.prototype.clone();
        for _ in 0..MAX_PROTOTYPE_DEPTH {
            current = match current {
                // The marker's own realm travels with the table it names. An ordinary object
                // has no realm of its own to ask -- `value_realm` of one is always the primary
                // -- so the chain is the only thing that knows which realm's methods this
                // object inherits. Dropping it here made every object built in a second realm
                // read the *first* realm's methods off its own prototype.
                Some(Value::Native(Native::PrototypeOf(table), realm)) => {
                    return Some((table, Some(realm)));
                }
                Some(Value::Object(parent)) => parent.borrow().prototype.clone(),
                Some(Value::Function(parent)) => parent.properties.borrow().prototype.clone(),
                // An array's own chain ends at `Array.prototype` unless the program moved it,
                // which is where an object linked to an array finds the array methods. There is
                // no marker to read a realm off, so the caller's own answer stands.
                Some(Value::Array(items)) => match items.properties.borrow().prototype.clone() {
                    Some(moved) => Some(moved),
                    None => return Some((PrototypeTable::Array, None)),
                },
                _ => return None,
            };
        }
        None
    }

    /// A property slot, following the prototype chain.
    ///
    /// Iterative and bounded rather than recursive. A chain is built by the program, so its
    /// length is the program's business -- but the recursion was on the *Rust* stack, where
    /// running out is an abort rather than an error, and a chain long enough to do that is
    /// reachable from ordinary code.
    pub fn lookup(&self, key: &str) -> Option<Property> {
        self.lookup_key(PropertyKeyRef::Utf8(key))
    }

    pub fn lookup_units(&self, key: &StringUnits) -> Option<Property> {
        self.lookup_key(PropertyKeyRef::Units(key))
    }

    pub(super) fn lookup_key(&self, key: PropertyKeyRef<'_>) -> Option<Property> {
        if let Some((index, own)) = self
            .properties
            .iter()
            .enumerate()
            .find(|(_, slot)| key.matches_slot(slot))
        {
            return Some(self.property_at_slot(index, own));
        }
        let mut current = self.prototype.clone();
        for _ in 0..MAX_PROTOTYPE_DEPTH {
            let found = match &current {
                // An array in a prototype chain contributes its elements and its `length` as
                // well as its property store. `function f() {}; f.prototype = [1]` is how the
                // corpus subclasses an array, and without this the instance inherits neither
                // the element nor the length -- so every generic method reached through it
                // walked an object that looked empty.
                Some(Value::Array(items)) => {
                    let store = items.properties.borrow();
                    store
                        .properties
                        .iter()
                        .find(|slot| key.matches_slot(slot))
                        .map(|slot| slot.property.clone())
                        .ok_or_else(|| store.prototype.clone())
                }
                Some(Value::Object(parent)) => {
                    let parent = parent.borrow();
                    parent
                        .properties
                        .iter()
                        .enumerate()
                        .find(|(_, slot)| key.matches_slot(slot))
                        .map(|(index, slot)| parent.property_at_slot(index, slot))
                        .ok_or_else(|| parent.prototype.clone())
                }
                Some(Value::Function(parent)) => {
                    let parent = parent.properties.borrow();
                    parent
                        .properties
                        .iter()
                        .find(|slot| key.matches_slot(slot))
                        .map(|slot| slot.property.clone())
                        .ok_or_else(|| parent.prototype.clone())
                }
                _ => return None,
            };
            match found {
                Ok(property) => return Some(property),
                Err(next) => current = next,
            }
        }
        None
    }

    /// Define an own data property, replacing whatever was there.
    ///
    /// Unlike [`ObjectData::set`] this never consults an inherited accessor: `defineProperty`
    /// *defines*, where an assignment *assigns*, and the difference is exactly the idiom where a
    /// prototype getter replaces itself with an own value on first read. Routing that through
    /// the prototype's setter calls it again, and again.
    pub fn define_value(&mut self, key: &str, value: Value, attributes: Attributes) {
        self.define_value_key(PropertyKeyRef::Utf8(key), value, attributes);
    }

    pub fn define_value_units(&mut self, key: &StringUnits, value: Value, attributes: Attributes) {
        self.define_value_key(PropertyKeyRef::Units(key), value, attributes);
    }

    pub(super) fn define_value_key(
        &mut self,
        key: PropertyKeyRef<'_>,
        value: Value,
        attributes: Attributes,
    ) {
        if let Some(slot) = self
            .properties
            .iter_mut()
            .find(|slot| key.matches_slot(slot))
        {
            if !redefinable(slot, attributes, true) {
                return;
            }
            slot.property = Property::Value(value);
            slot.attributes = attributes;
            return;
        }
        if !self.extensible {
            return;
        }
        self.insert_key(key, Property::Value(value), attributes);
    }

    /// Change an existing property's flags without touching what it holds.
    ///
    /// `Object.defineProperty(o, "a", { enumerable: false })` names no value and means the one
    /// it already has; reading the value out and writing it back would be the same thing for a
    /// data property and would *call the getter* for an accessor.
    pub fn set_attributes(&mut self, key: &str, attributes: Attributes) {
        if let Some(slot) = self.properties.iter_mut().find(|slot| slot.key == key) {
            let data = matches!(slot.property, Property::Value(_));
            if redefinable(slot, attributes, data) {
                slot.attributes = attributes;
            }
        }
    }

    /// Define an accessor, merging with a half already defined under the same key.
    ///
    /// Merging matters because `get x()` and `set x()` are written separately and describe one
    /// property; defining the second must not discard the first.
    pub fn define_accessor(
        &mut self,
        key: &str,
        getter: Option<Value>,
        setter: Option<Value>,
        attributes: Attributes,
    ) {
        self.define_accessor_key(PropertyKeyRef::Utf8(key), getter, setter, attributes);
    }

    /// Exact-unit counterpart of [`ObjectData::define_accessor`].
    pub fn define_accessor_units(
        &mut self,
        key: &StringUnits,
        getter: Option<Value>,
        setter: Option<Value>,
        attributes: Attributes,
    ) {
        self.define_accessor_key(PropertyKeyRef::Units(key), getter, setter, attributes);
    }

    pub(super) fn define_accessor_key(
        &mut self,
        key: PropertyKeyRef<'_>,
        getter: Option<Value>,
        setter: Option<Value>,
        attributes: Attributes,
    ) {
        let existing = match self.get_own_property_key(key) {
            Some(Property::Accessor { get, set }) => (get.clone(), set.clone()),
            _ => (None, None),
        };
        let property = Property::Accessor {
            get: getter.or(existing.0),
            set: setter.or(existing.1),
        };
        if let Some(slot) = self
            .properties
            .iter_mut()
            .find(|slot| key.matches_slot(slot))
        {
            if !redefinable(slot, attributes, false) {
                return;
            }
            slot.property = property;
            slot.attributes = attributes;
            return;
        }
        self.properties.push(Slot {
            key: key.owned(),
            symbol: key.is_symbol(),
            internal: key.is_internal(),
            property,
            attributes,
        });
    }

    pub(super) fn install_key(
        &mut self,
        key: PropertyKeyRef<'_>,
        property: Property,
        attributes: Attributes,
    ) {
        if let Some(slot) = self
            .properties
            .iter_mut()
            .find(|slot| key.matches_slot(slot))
        {
            slot.property = property;
            slot.attributes = attributes;
            return;
        }
        self.insert_key(key, property, attributes);
    }

    /// A property, following the prototype chain.
    ///
    /// Returns an owned value rather than a reference because the answer may live in another
    /// object behind an `Rc<RefCell<_>>`, whose borrow cannot outlive this call.
    pub fn get(&self, key: &str) -> Option<Value> {
        // Same walk as `lookup`, and data-only: this is what the VM's own reads use.
        match self.lookup(key) {
            Some(Property::Value(value)) => Some(value),
            _ => None,
        }
    }

    pub fn get_units(&self, key: &StringUnits) -> Option<Value> {
        match self.lookup_units(key) {
            Some(Property::Value(value)) => Some(value),
            _ => None,
        }
    }

    pub fn prototype(&self) -> Option<&Value> {
        self.prototype.as_ref()
    }

    /// Link this object to a prototype, refusing to close a cycle.
    ///
    /// The specification makes `Object.setPrototypeOf` a TypeError when it would create one.
    /// Here the reason is more immediate: a cycle makes every miss walk forever.
    pub fn set_prototype(&mut self, prototype: Value) {
        let mut current = Some(prototype.clone());
        for _ in 0..MAX_PROTOTYPE_DEPTH {
            let next = match &current {
                Some(Value::Object(parent)) => {
                    if std::ptr::eq(parent.as_ptr(), std::ptr::from_ref(self)) {
                        return;
                    }
                    parent.borrow().prototype.clone()
                }
                Some(Value::Function(parent)) => parent.properties.borrow().prototype.clone(),
                _ => break,
            };
            current = next;
        }
        self.prototype = Some(prototype);
    }

    /// Insert or overwrite, keeping the store in enumeration order.
    ///
    /// Re-assigning a property does not move it to the end in JavaScript, so it must not here.
    /// A *new* key is placed where enumeration would put it rather than simply appended: an
    /// array index comes before every string key, in ascending numeric order, however it was
    /// written. Ordering on insert rather than on read means `Object.keys`, `JSON.stringify`,
    /// spread and `for-in` are all correct without any of them knowing the rule.
    pub fn set(&mut self, key: &str, value: Value) {
        self.set_key(PropertyKeyRef::Utf8(key), value);
    }

    pub fn set_units(&mut self, key: &StringUnits, value: Value) {
        self.set_key(PropertyKeyRef::Units(key), value);
    }

    pub(super) fn set_key(&mut self, key: PropertyKeyRef<'_>, value: Value) {
        if self.frozen || self.module_namespace {
            return;
        }

        if let Some(slot) = self
            .properties
            .iter_mut()
            .find(|slot| key.matches_slot(slot))
        {
            // Only a data property can refuse a write. An accessor's setter has already had its
            // chance -- `inherited_setter_key` runs ahead of every store -- so a store that reaches
            // an accessor slot here is the machine replacing its own bookkeeping.
            if matches!(slot.property, Property::Value(_)) && !slot.attributes.writable {
                return;
            }
            slot.property = Property::Value(value);
            return;
        }
        if !self.extensible {
            return;
        }
        self.insert_key(key, Property::Value(value), Attributes::DEFAULT);
    }

    pub(super) fn insert_key(
        &mut self,
        key: PropertyKeyRef<'_>,
        property: Property,
        attributes: Attributes,
    ) {
        let slot = Slot {
            key: key.owned(),
            symbol: key.is_symbol(),
            internal: key.is_internal(),
            property,
            attributes,
        };
        if slot.symbol {
            self.properties.push(slot);
            return;
        }
        let Some(index) = index_key_units(&slot.key) else {
            self.properties.push(slot);
            return;
        };
        // The indices already form a sorted prefix, so this is where the new one joins it.
        let position = self
            .properties
            .iter()
            .position(|other| index_key_units(&other.key).is_none_or(|other| other > index))
            .unwrap_or(self.properties.len());
        self.properties.insert(position, slot);
    }

    /// Overwrite what one export of a module namespace holds, leaving its flags alone.
    ///
    /// Every ordinary write to a namespace is refused, which is what the object is for -- so the
    /// one write that is not the program's has to come in by its own door. The linker uses it to
    /// keep a namespace a module imported *from itself* in step with the bindings it shows: the
    /// object has to exist before the body that fills those bindings runs, so it is built over
    /// the slots and caught up from them rather than snapshotted after the fact.
    pub fn refresh_module_namespace_export(&mut self, key: &str, value: Value) {
        if let Some(slot) = self
            .properties
            .iter_mut()
            .find(|slot| !slot.symbol && slot.key == key)
        {
            slot.property = Property::Value(value);
        }
    }

    /// Refuse every further change to this object.
    pub fn freeze(&mut self) {
        self.frozen = true;
        self.extensible = false;
        // Freezing is a statement about every property, not only about the shape: a descriptor
        // read afterwards has to report each one as non-writable and non-configurable, which it
        // can only do if freezing actually wrote that down.
        for slot in &mut self.properties {
            slot.attributes.writable = false;
            slot.attributes.configurable = false;
        }
    }

    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Whether this is a module namespace, whose `[[Set]]` refuses.
    pub fn is_module_namespace(&self) -> bool {
        self.module_namespace
    }

    /// Close a module namespace: no new properties, and no write to an existing one.
    ///
    /// Deliberately weaker than [`ObjectData::freeze`] in what it *says* and exactly as strong
    /// in what it *does*. A namespace's exports are writable and non-configurable, so a program
    /// reading a descriptor is told the truth, and `Object.isFrozen` of one is false; the write
    /// is refused all the same, because `[[Set]]` on a namespace is defined to return false
    /// whatever the flags say.
    pub fn close_as_module_namespace(&mut self) {
        self.extensible = false;
        self.module_namespace = true;
    }

    /// Record the cell and immutable property slot an export is a second name for.
    pub fn track_namespace_cell(&mut self, key: &str, cell: Value) {
        if matches!(cell, Value::Cell(_)) {
            // Construction has closed the namespace before tracking cells, so later reads can
            // reuse immutable indices even when numeric export names reordered earlier slots.
            // The last-slot shortcut and one-time search also preserve duplicate-name inputs;
            // refreshing never searches by name.
            let index = self
                .properties
                .last()
                .filter(|slot| slot.key == key)
                .map(|_| self.properties.len() - 1)
                .or_else(|| self.properties.iter().position(|slot| slot.key == key));
            if let Some(index) = index {
                // The cell is the sole owner of the live export value. Keeping the initial
                // snapshot in the ordinary property slot would retain replaced objects.
                self.properties[index].property = Property::Value(Value::Undefined);
                let bindings = self
                    .namespace_bindings
                    .get_or_insert_with(|| Box::default());
                if bindings.cell_slots.len() < self.properties.len() {
                    bindings.cell_slots.resize(self.properties.len(), None);
                }
                if let Some(cell_index) = bindings.cell_slots[index] {
                    // A duplicate name in direct construction replaces the property and its
                    // binding. Reuse that entry so the superseded cell is not retained.
                    bindings.cells[cell_index] = cell;
                } else {
                    let cell_index = bindings.cells.len();
                    bindings.cells.push(cell);
                    bindings.cell_slots[index] = Some(cell_index);
                }
            }
        }
    }

    /// Resolve a stored property slot through its live export cell, if it has one.
    pub(super) fn property_at_slot(&self, index: usize, slot: &Slot) -> Property {
        if let Some(Value::Cell(cell)) = self
            .namespace_bindings
            .as_ref()
            .and_then(|bindings| bindings.cell_slots.get(index))
            .and_then(|cell_index| *cell_index)
            .and_then(|cell_index| {
                self.namespace_bindings
                    .as_ref()
                    .and_then(|bindings| bindings.cells.get(cell_index))
            })
        {
            return Property::Value(read_through_cell(&Value::Cell(Rc::clone(cell))));
        }
        slot.property.clone()
    }

    /// Whether this object is a namespace built over live bindings, and so needs catching up.
    pub fn tracks_namespace_cells(&self) -> bool {
        self.namespace_bindings
            .as_ref()
            .is_some_and(|bindings| !bindings.cells.is_empty())
    }

    /// Refuse new properties, leaving the existing ones writable.
    pub fn prevent_extensions(&mut self) {
        self.extensible = false;
    }

    /// `Object.seal`: refuse new properties and refuse to remove or redefine the existing ones,
    /// but leave them writable. Weaker than [`ObjectData::freeze`] by exactly that one flag.
    pub fn seal(&mut self) {
        self.extensible = false;
        for slot in &mut self.properties {
            slot.attributes.configurable = false;
        }
    }

    /// Whether nothing can be added to this object and nothing removed from it.
    ///
    /// Not simply "not extensible": a property defined as configurable before the object was
    /// closed can still be deleted, and an object one `delete` away from a different shape is
    /// not sealed.
    pub fn is_sealed(&self) -> bool {
        !self.extensible
            && self
                .properties
                .iter()
                .all(|slot| !slot.attributes.configurable)
    }

    pub fn is_extensible(&self) -> bool {
        self.extensible
    }

    /// Remove an own property, keeping the order of the rest.
    pub fn remove(&mut self, key: &str) {
        self.remove_key(PropertyKeyRef::Utf8(key));
    }

    pub fn remove_units(&mut self, key: &StringUnits) -> bool {
        self.remove_key(PropertyKeyRef::Units(key))
    }

    pub(super) fn remove_key(&mut self, key: PropertyKeyRef<'_>) -> bool {
        if self
            .properties
            .iter()
            .find(|slot| key.matches_slot(slot))
            .is_some_and(|slot| !slot.attributes.configurable)
        {
            return false;
        }
        self.properties
            .retain(|slot| !key.matches_slot(slot) || !slot.attributes.configurable);

        true
    }

    pub fn keys(&self) -> impl Iterator<Item = &StringUnits> {
        self.properties
            .iter()
            .filter(|slot| !slot.symbol && !slot.internal)
            .map(|slot| &slot.key)
    }

    /// Own *enumerable* property slots in insertion order, which is the order JavaScript
    /// enumerates them in.
    pub fn entries(&self) -> impl Iterator<Item = (&StringUnits, &Property)> {
        self.properties
            .iter()
            .filter(|slot| slot.attributes.enumerable && !slot.internal)
            .map(|slot| (&slot.key, &slot.property))
    }

    /// Enumerable entries with namespace exports resolved from their current cells.
    pub(super) fn resolved_entries(&self) -> Vec<(StringUnits, Property)> {
        self.properties
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.attributes.enumerable && !slot.internal)
            .map(|(index, slot)| (slot.key.clone(), self.property_at_slot(index, slot)))
            .collect()
    }

    pub(super) fn resolved_string_entries(&self) -> Vec<(StringUnits, Property)> {
        self.properties
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.attributes.enumerable && !slot.symbol && !slot.internal)
            .map(|(index, slot)| (slot.key.clone(), self.property_at_slot(index, slot)))
            .collect()
    }

    pub(super) fn string_entries(&self) -> impl Iterator<Item = (&StringUnits, &Property)> {
        self.properties
            .iter()
            .filter(|slot| slot.attributes.enumerable && !slot.symbol && !slot.internal)
            .map(|slot| (&slot.key, &slot.property))
    }

    /// Define a property that reads by name but is never enumerated, which is what every own
    /// property of an error is: `e.message` works and `Object.keys(e)` is empty.
    pub fn set_non_enumerable(&mut self, key: &str, value: Value) {
        self.define_value(key, value, Attributes::BUILT_IN);
    }
}

/// Whether an existing slot may be redefined with these flags.
///
/// A configurable property may be redefined however the program likes. A non-configurable one
/// may not -- with the single exception the specification carves out: a non-configurable but
/// *writable* data property may still have its value replaced, and may be made non-writable,
/// because neither is something an observer holding the descriptor was promised would not
/// happen. The refusal is silent because the callers are the machine's own definitions -- an
/// object literal, a class body, a built-in laying out its properties -- none of which is a
/// program asking a question it can be answered. `Object.defineProperty` *is* such a question,
/// and it does not come through here: [`define_own_property_key`] runs the specification's
/// `ValidateAndApplyPropertyDescriptor` and reports the refusal, which `DefinePropertyOrThrow`
/// then raises in both strict and sloppy mode.
pub(super) fn redefinable(slot: &Slot, attributes: Attributes, data: bool) -> bool {
    slot.attributes.configurable
        || (data
            && matches!(slot.property, Property::Value(_))
            && slot.attributes.writable
            && attributes.enumerable == slot.attributes.enumerable
            && !attributes.configurable)
}

/// A property descriptor as `ToPropertyDescriptor` reads one: six fields, each of which may be
/// absent.
///
/// Absence is the information. `{ value: 1 }` on a fresh key locks every axis it did not name,
/// while `{ enumerable: false }` on an existing one changes exactly that flag -- so a field that
/// is missing and a field holding `undefined` are deliberately different things, which is why
/// none of these collapses into a plain value.
#[derive(Clone, Debug, Default)]
pub(super) struct Descriptor {
    pub(super) value: Option<Value>,
    pub(super) get: Option<Value>,
    pub(super) set: Option<Value>,
    pub(super) writable: Option<bool>,
    pub(super) enumerable: Option<bool>,
    pub(super) configurable: Option<bool>,
}

impl Descriptor {
    pub(super) fn is_accessor(&self) -> bool {
        self.get.is_some() || self.set.is_some()
    }

    pub(super) fn is_data(&self) -> bool {
        self.value.is_some() || self.writable.is_some()
    }

    /// Neither kind: flags only, which may be applied to a property of either kind.
    pub(super) fn is_generic(&self) -> bool {
        !self.is_accessor() && !self.is_data()
    }
}

/// Whether this is an Object in the specification's sense -- which is every value that is not
/// one of the seven primitive types.
///
/// A function, an array, a date, a proxy and a built-in namespace are all objects; the check is
/// written as the complement because the list of things that are *not* is the short and closed
/// one.
pub(super) fn is_an_object(value: &Value) -> bool {
    !matches!(
        value,
        Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::BigInt(_)
            | Value::Symbol(_)
    )
}

/// Whether a new property may be added to this value.
///
/// A built-in is an ordinary extensible object in the specification, and the realm's store is
/// where a program's `preventExtensions` on one is recorded -- so this asks the store rather
/// than answering `false` for everything it has no slot for. Test262 checks the claim on every
/// built-in it reaches: "Built-in objects must be extensible" is its own assertion.
/// The store an exotic object keeps its ordinary named properties in.
///
/// A typed array, a buffer, a view and a regexp are each exotic in exactly *one* respect -- the
/// elements a view answers for itself, the bytes a buffer holds, the search position a regexp
/// carries -- and an ordinary object in every other. `sample.constructor = C` is not a special
/// case of anything: it is a property write, and while there was nowhere to put one the write
/// was dropped and the read afterwards answered the intrinsic the program had just replaced.
///
/// One function rather than a match arm per operation, so that a read, a write, `in`, `delete`,
/// a descriptor, an own-key walk and a definition cannot disagree about where a name lives.
pub(super) fn exotic_properties(target: &Value) -> Option<&RefCell<ObjectData>> {
    Some(match target {
        Value::TypedArray(view) => &view.properties,
        Value::ArrayBuffer(buffer) => &buffer.properties,
        Value::DataView(view) => &view.properties,
        Value::RegExp(regexp) => &regexp.properties,
        Value::Date(date) => &date.properties,
        Value::Temporal(built) => &built.properties,
        Value::Promise(promise) => &promise.properties,
        Value::Generator(generator) => &generator.properties,
        // A wrapped function is exotic in exactly one respect -- what calling it does -- and an
        // ordinary object in every other: it starts out holding the `name` and `length` copied
        // off its target, and a holder may add to it. Its store is its own and is never the
        // target's, which is what "wrapped functions share no properties" means: a write here
        // must not reach across the boundary, and a property the target has must not appear.
        Value::Wrapped(wrapped) => &wrapped.properties,
        _ => return None,
    })
}

/// The store a value records a `[[Prototype]]` in, or `None` when its chain is fixed by its kind.
///
/// Every value with somewhere to put ordinary named properties also has somewhere to put a
/// prototype, and the two are the same store. Naming it once is what keeps the six places that
/// *write* a chain -- `Object.setPrototypeOf`, `Reflect.setPrototypeOf`, `__proto__` either way
/// round, `class D extends B`, and `super()` handing back the parent's object -- agreeing with
/// the one place that reads it. They did not: a write reached an object, a function and an
/// array, and the read looked only there, so `class V extends Uint8Array {}` produced a view
/// whose prototype was the intrinsic one and `new V() instanceof V` was false.
pub(super) fn prototype_slot(target: &Value) -> Option<&RefCell<ObjectData>> {
    match target {
        Value::Object(object) => Some(object.as_ref()),
        Value::Array(items) => Some(&items.properties),
        Value::Function(closure) => Some(&closure.properties),
        Value::Bound(bound) => Some(&bound.properties),
        other => exotic_properties(other),
    }
}

/// Record `prototype` as the value's own chain, reporting whether it had anywhere to keep one.
///
/// The refusal is the answer `Reflect.setPrototypeOf` gives, and the reason `super()` leaves an
/// exotic parent's object alone when it has no slot: a link that cannot be stored must not be
/// reported as stored.
pub(super) fn record_prototype(target: &Value, prototype: Value) -> bool {
    let Some(slot) = prototype_slot(target) else {
        return false;
    };
    slot.borrow_mut().set_prototype(prototype);
    true
}

/// Whether the value answers this name from its own exotic behaviour rather than from the store
/// beside it.
///
/// For a typed array the line is `CanonicalNumericIndexString`, which is the specification's own
/// and is wider than "an index": `"1.5"` and `"-0"` reach no element and are still the view's to
/// answer, so a write to one is discarded rather than stored, while `"01"` is an ordinary name
/// and is stored. A regexp claims `lastIndex`, which lives in a slot of its own so that a search
/// and a read of it cannot disagree about where the next match starts.
pub(super) fn exotic_reserved_name(target: &Value, key: &str) -> bool {
    exotic_reserved_name_key(target, PropertyKeyRef::Utf8(key))
}

pub(super) fn exotic_reserved_name_key(target: &Value, key: PropertyKeyRef<'_>) -> bool {
    match target {
        Value::TypedArray(_) => canonical_numeric_index_key(key).is_some(),
        Value::RegExp(_) => false,
        _ => false,
    }
}

pub(super) fn remove_exotic_property_key(
    store: &RefCell<ObjectData>,
    key: PropertyKeyRef<'_>,
) -> bool {
    if store
        .borrow()
        .get_own_attributes_key(key)
        .is_some_and(|attributes| !attributes.configurable)
    {
        return false;
    }
    store.borrow_mut().remove_key(key);
    true
}

pub(super) fn is_extensible_target(intrinsics: &Intrinsics, target: &Value) -> bool {
    match target {
        Value::Shared(_) => false,
        Value::Object(object) => object.borrow().is_extensible(),
        Value::Function(closure) => closure.properties.borrow().is_extensible(),
        Value::Array(items) => items.properties.borrow().is_extensible(),
        Value::Bound(bound) => bound.properties.borrow().is_extensible(),
        Value::Native(_, _)
        | Value::Callback(_, _)
        | Value::Settler(..)
        | Value::Capability(..)
        | Value::Element(_) => intrinsics.extensible(target),
        // An exotic object carries an ordinary store for its named properties, and closing it is
        // what `Object.preventExtensions(sample)` has to mean: the elements are not properties
        // of that store and are unaffected, which is what the specification says too.
        other if exotic_properties(other).is_some() => {
            exotic_properties(target).is_none_or(|store| store.borrow().is_extensible())
        }
        // Everything left is a primitive or a shape with no store at all. A definition on one is
        // dropped rather than refused, and reporting it closed would turn that silence into a
        // throw the specification does not ask for.
        _ => true,
    }
}

/// What an own property holds and how it behaves, or `None` when there is no such property.
///
/// The pair `ValidateAndApplyPropertyDescriptor` compares a descriptor against: the flags alone
/// cannot answer whether a non-configurable property is being redefined to the value it already
/// has, which the specification permits.
pub(super) fn own_property_state(
    intrinsics: &Intrinsics,
    target: &Value,
    key: &str,
) -> Option<(Property, Attributes)> {
    own_property_state_key(intrinsics, target, PropertyKeyRef::Utf8(key))
}

pub(super) fn own_property_state_key(
    intrinsics: &Intrinsics,
    target: &Value,
    key: PropertyKeyRef<'_>,
) -> Option<(Property, Attributes)> {
    // A namespace export's slot carries the ordinary property shape, while its value lives in
    // the exporting module's cell. Resolve just this slot here so [[GetOwnProperty]], proxy
    // invariants, descriptors and the ordinary Get walk all observe the same current binding.
    if let Value::Object(object) = target {
        let data = object.borrow();
        if let Some((index, slot)) = data
            .properties
            .iter()
            .enumerate()
            .find(|(_, slot)| key.matches_slot(slot))
        {
            return Some((data.property_at_slot(index, slot), slot.attributes));
        }
    }
    let attributes = own_attributes_key(intrinsics, target, key)?;
    let property = match target {
        Value::Shared(facade) => facade.own_property(key).map(|(property, _)| property),
        Value::Object(_) => string_exotic_key(target, key).map(Property::Value),
        Value::Function(closure)
            if key.matches_ascii("prototype") && has_prototype_property(closure) =>
        {
            Some(Property::Value(prototype_object(closure)))
        }
        Value::Function(closure) => closure
            .properties
            .borrow()
            .get_own_property_key(key)
            .cloned(),
        Value::Array(items) => match array_index_key(key) {
            Some(index) => match array_exotic_element(items, index) {
                Some(entry) => Some(entry.property),
                None => items.borrow().get(index).cloned().map(Property::Value),
            },
            // `length` is an own property, and derived rather than stored: the elements are the
            // only record of how many there are, so it is read off them here rather than kept
            // beside them where the two could disagree.
            None if key.matches_ascii("length") => array_own_property_key(items, key),
            None => items.properties.borrow().get_own_property_key(key).cloned(),
        },
        // An element key is the view's own answer and a named one is the store's, and which is
        // which is `CanonicalNumericIndexString` rather than "is it an index": `"1.5"` names no
        // element and is still not a property, while `"01"` is an ordinary name.
        Value::TypedArray(view) => match canonical_numeric_index_key(key) {
            Some(index) => valid_integer_index(view, index)
                .and_then(|at| view.get(at))
                .map(Property::Value),
            None => view.properties.borrow().get_own_property_key(key).cloned(),
        },
        // A buffer, a view and a regexp have no exotic keys at all: everything they publish is
        // an accessor on the prototype, so every own property of one is a stored one.
        other if exotic_properties(other).is_some() => exotic_properties(target)
            .and_then(|store| store.borrow().get_own_property_key(key).cloned()),
        // A built-in keeps what a program changed in the realm's store and everything else in
        // the tables, so a property it has never touched still reads as the specification built
        // it. `own_attributes` has already decided which of the two is answering.
        Value::Bound(bound) => bound
            .properties
            .borrow()
            .get_own_property_key(key)
            .cloned()
            .or_else(|| tabulated_property_key(intrinsics, target, key)),
        Value::Native(_, _)
        | Value::Callback(_, _)
        | Value::Settler(..)
        | Value::Capability(..)
        | Value::Element(_) => {
            match intrinsics.overridden_key(target, key) {
                Overridden::Held(property, _) => Some(property),
                // Unreachable: `own_attributes` answered `None` for a removed property, so the
                // `?` above already returned.
                Overridden::Absent => None,
                Overridden::Untouched => tabulated_property_key(intrinsics, target, key),
            }
        }
        // A string's characters, which are own properties of the primitive itself. Reached from
        // `own_descriptor_key`; a definition never gets here, because a primitive is refused before
        // its key is even converted.
        Value::String(text) if key.matches_ascii("length") => {
            Some(Property::Value(Value::Number(text.unit_len() as f64)))
        }
        Value::String(text) => array_index_key(key)
            .and_then(|index| text.unit_at(index))
            .map(|unit| Property::Value(Value::String(StringUnits::from_units(&[unit])))),
        _ => None,
    }?;
    Some((property, attributes))
}

/// What a built-in's own property held when the realm was built.
pub(super) fn tabulated_property(
    intrinsics: &Intrinsics,
    target: &Value,
    key: &str,
) -> Option<Property> {
    tabulated_property_key(intrinsics, target, PropertyKeyRef::Utf8(key))
}

pub(super) fn tabulated_property_key(
    intrinsics: &Intrinsics,
    target: &Value,
    key: PropertyKeyRef<'_>,
) -> Option<Property> {
    let key = key.known_ascii()?;
    let key = key.as_ref();
    let realm = value_realm(target);
    // `String.prototype` is the empty String exotic object. Its locked own `length` lives in
    // the boxed value rather than Native's method table, but materializing the intrinsic must
    // copy it just like every tabulated property; otherwise the complete store records the key
    // as absent and a prior `Object.getOwnPropertyNames(String.prototype)` erases the next read.
    if let Some(value) = string_exotic(target, key) {
        return Some(Property::Value(value));
    }
    // An accessor is asked about first, because one of them is named `length`:
    // `%TypedArray%.prototype.length` is a getter over the view's element count, and reading it
    // as the arity of a callable answered `None` for a property that is really there.
    if let Some(accessor) = intrinsic_accessor(target, key) {
        return Some(accessor);
    }
    if let Some(accessor) = machine_accessor(target, key) {
        return Some(Property::Accessor {
            get: Some(Value::Native(Native::BuiltinAccessor(accessor), realm)),
            set: None,
        });
    }
    match key {
        "name" => {
            intrinsic_identity(target).map(|(name, _)| Property::Value(Value::String(name.into())))
        }
        "length" => {
            if matches!(
                target,
                Value::Native(Native::PrototypeOf(PrototypeTable::Array), _)
            ) {
                Some(Property::Value(Value::Number(0.0)))
            } else {
                intrinsic_identity(target).map(|(_, length)| Property::Value(Value::Number(length)))
            }
        }
        "prototype" => intrinsic_prototype_in(intrinsics, target).map(Property::Value),
        _ if matches!(
            target,
            Value::Native(Native::PrototypeOf(PrototypeTable::Array), _)
        ) && key == WellKnown::Unscopables.property_key() =>
        {
            Some(Property::Value(intrinsics.array_unscopables(realm)))
        }
        _ => match target {
            Value::Native(native, _) => match native.property(key) {
                Some(Value::Undefined) | None => None,
                Some(found) => Some(Property::Value(found)),
            },
            _ => None,
        },
    }
}

/// What the slot holds after a definition: the descriptor's own fields, and for each one it does
/// not name, whatever was there before.
pub(super) fn descriptor_property(descriptor: &Descriptor, current: Option<Property>) -> Property {
    if descriptor.is_accessor() {
        // `{ get }` over an existing accessor keeps its setter: the two halves are written
        // separately and describe one property, so naming one must not discard the other.
        let (kept_get, kept_set) = match &current {
            Some(Property::Accessor { get, set }) => (get.clone(), set.clone()),
            _ => (None, None),
        };
        let half = |named: &Option<Value>, kept: Option<Value>| match named {
            // A half named as `undefined` is a half removed, not a half left alone.
            Some(Value::Undefined) => None,
            Some(function) => Some(function.clone()),
            None => kept,
        };
        return Property::Accessor {
            get: half(&descriptor.get, kept_get),
            set: half(&descriptor.set, kept_set),
        };
    }
    if descriptor.is_data() {
        // A data descriptor over an accessor replaces it outright, so the value it does not name
        // is `undefined` rather than whatever the getter would have answered.
        let kept = match &current {
            Some(Property::Value(value)) => Some(value.clone()),
            _ => None,
        };
        return Property::Value(
            descriptor
                .value
                .clone()
                .or(kept)
                .unwrap_or(Value::Undefined),
        );
    }
    // Flags only. Reading the value out and writing it back would be the same thing for a data
    // property and would *call the getter* for an accessor, so the property is left as it is.
    current.unwrap_or(Property::Value(Value::Undefined))
}

pub(super) fn install_property_key(
    intrinsics: &Intrinsics,
    target: &Value,
    key: PropertyKeyRef<'_>,
    property: Property,
    attributes: Attributes,
) -> Result<(), ExecutionError> {
    match target {
        Value::Object(object) => object.borrow_mut().install_key(key, property, attributes),
        Value::Function(closure) => {
            if key.matches_ascii("prototype")
                && has_prototype_property(closure)
                && let Property::Value(value) = &property
            {
                *closure.prototype.borrow_mut() = Some(value.clone());
            }
            closure
                .properties
                .borrow_mut()
                .install_key(key, property, attributes);
        }
        Value::Bound(bound) => bound
            .properties
            .borrow_mut()
            .install_key(key, property, attributes),
        Value::Array(items) => match array_index_key(key) {
            // Bounded like the ordinary indexed write: `defineProperty` reaches the same dense
            // storage and can name just as distant an index.
            Some(index) if index >= MAX_DENSE_ARRAY => {
                return Err(ExecutionError::ArrayTooLarge { length: index + 1 });
            }
            Some(index) => {
                {
                    let mut elements = items.borrow_mut();
                    if index >= elements.len() {
                        items.sparse.set(true);
                        elements.resize(index + 1, Value::Native(Native::Hole, RealmId::PRIMARY));
                    }
                    // A data property keeps its value in the dense storage whatever its flags,
                    // so every reader that walks the elements still sees it. An accessor leaves
                    // a hole there: the storage holds values and nothing else, and the property
                    // itself is recorded beside it.
                    elements[index] = match &property {
                        Property::Value(value) => value.clone(),
                        Property::Accessor { .. } => {
                            items.sparse.set(true);
                            Value::Native(Native::Hole, RealmId::PRIMARY)
                        }
                    };
                }
                let ordinary =
                    matches!(property, Property::Value(_)) && attributes == Attributes::DEFAULT;
                let mut exotic = items.elements.borrow_mut();
                exotic.retain(|entry| entry.index != index);
                if !ordinary {
                    exotic.push(ExoticElement {
                        index,
                        property,
                        attributes,
                    });
                }
            }
            None => items
                .properties
                .borrow_mut()
                .install_key(key, property, attributes),
        },
        // A built-in's own properties live in the realm's store, which is the only place a
        // change to one can go: the tables it is otherwise answered from are `match` arms.
        Value::Native(_, _) | Value::Callback(_, _) => {
            intrinsics.install_key(target, key, property, attributes);
        }
        // An exotic object takes an ordinary named property in the store beside its own state.
        // An element key never reaches here: `define_own_property_key` answers those itself, the
        // way it answers an array's `length` itself, because their flags are the specification's
        // rather than the store's.
        other if exotic_properties(other).is_some() => {
            if let Some(store) = exotic_properties(target)
                && !exotic_reserved_name_key(target, key)
            {
                store.borrow_mut().install_key(key, property, attributes);
            }
        }
        // A primitive cannot reach this: the caller rejected it before ToPropertyKey.
        _ => {}
    }
    Ok(())
}

/// Why a definition was refused, worded as node words it: the two reasons a program can hit are
/// a closed object and a property that promised not to change, and the message is the only thing
/// telling them apart once the `TypeError` is caught.
pub(super) fn refusal(intrinsics: &Intrinsics, target: &Value, key: &str) -> String {
    if own_attributes(intrinsics, target, key).is_some() {
        format!("Cannot redefine property: {key}")
    } else {
        format!("Cannot define property {key}, object is not extensible")
    }
}

/// `[[DefineOwnProperty]]` for the ordinary shapes: the specification's
/// `ValidateAndApplyPropertyDescriptor` followed by the write it approved.
///
/// Answers with a boolean rather than throwing because its two callers want opposite things:
/// `Reflect.defineProperty` reports the refusal as `false`, and `Object.defineProperty` -- which
/// is `DefinePropertyOrThrow` -- raises it, in strict and sloppy mode alike.
/// `ArraySetLength(A, Desc)`: an array's `[[DefineOwnProperty]]` for its own `length`.
///
/// Not the ordinary validation with a different store behind it. The value is coerced twice --
/// once as `ToUint32` and once as `ToNumber` -- and a length that survives one but not the other
/// is a `RangeError` rather than a refusal, which is the only place in the language where
/// defining a property raises something that is not a `TypeError`. Shrinking then *deletes* the
/// elements above the new length, and stops at the first one that will not go.
pub(super) fn array_set_length(
    items: &Rc<ArrayData>,
    descriptor: &Descriptor,
) -> Result<bool, ExecutionError> {
    let Some(value) = descriptor.value.clone() else {
        // Flags only. `length` is non-configurable and never enumerable, so the only flag that
        // can move is `writable`, and only ever from true to false.
        if descriptor.configurable == Some(true) || descriptor.enumerable == Some(true) {
            return Ok(false);
        }
        if descriptor.is_accessor() {
            return Ok(false);
        }
        if descriptor.writable == Some(true) && !items.length_writable.get() {
            return Ok(false);
        }
        if descriptor.writable == Some(false) {
            items.length_writable.set(false);
        }
        return Ok(true);
    };
    // `ToUint32(value)` and `ToNumber(value)` disagreeing is what makes `{length: -1}` and
    // `{length: 2 ** 32}` refusals the program can catch as a `RangeError`.
    let number = value.as_number();
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 || number > MAX_ARRAY_LENGTH {
        return Err(ExecutionError::InvalidArrayLength);
    }
    if descriptor.configurable == Some(true)
        || descriptor.enumerable == Some(true)
        || descriptor.is_accessor()
    {
        return Ok(false);
    }
    #[expect(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let wanted = number as usize;
    let current = items.borrow().len();
    if !items.length_writable.get() && wanted != current {
        return Ok(false);
    }
    // A length past what the dense vector can hold is not an error: `length` is a count, not a
    // claim about storage, and `a.length = 4294967295` is a legal assignment that stores nothing.
    // Refusing it made every `verifyProperty(array, "length", ...)` in the test262 harness fail --
    // ninety-five rows across `Array`, `Object` and `Iterator` -- for an array of two elements.
    //
    // The elements are left alone and only the length moves: growing has never created elements
    // here, and an index past the end already reads as a hole.
    if wanted > MAX_DENSE_ARRAY {
        if !items.length_writable.get() {
            return Ok(false);
        }
        items.length.set(wanted);
        return Ok(true);
    }
    // The elements above the new length are deleted one at a time from the top, and the walk
    // stops at the first one that will not go: `length` ends up one past it, describing an array
    // that still matches what is in it rather than one the elements contradict.
    let frozen = items.properties.borrow().is_frozen();
    let mut reached = wanted;
    let mut index = current;
    while index > wanted {
        index -= 1;
        let defined = array_exotic_element(items, index);
        let present = defined.is_some()
            || items
                .borrow()
                .get(index)
                .is_some_and(|value| !matches!(value, Value::Native(Native::Hole, _)));
        let removable = match &defined {
            Some(entry) => entry.attributes.configurable && !frozen,
            None => !frozen,
        };
        if present && !removable {
            reached = index + 1;
            break;
        }
    }
    {
        let mut elements = items.borrow_mut();
        if reached <= elements.len() {
            elements.truncate(reached);
        } else {
            // Growing leaves holes, which read as undefined and are not the same thing.
            items.sparse.set(true);
            elements.resize(reached, Value::Native(Native::Hole, RealmId::PRIMARY));
        }
    }
    items
        .elements
        .borrow_mut()
        .retain(|entry| entry.index < reached);
    if descriptor.writable == Some(false) {
        items.length_writable.set(false);
    }
    Ok(reached == wanted)
}

/// What an integer-indexed `[[DefineOwnProperty]]` does with a descriptor.
///
/// Three answers rather than a boolean, because the middle one is a *call*: an accepted
/// definition ends in `ToNumber` -- or `ToBigInt` -- of the value, which is the program's own
/// method and may throw or detach the buffer it was aimed at. Naming the outcomes keeps the
/// refusals in one place, so the machine's spelling of this operation and the pure one cannot
/// disagree about which descriptors an element accepts.
pub(super) enum IndexedDefinition {
    /// The key reaches no live element, or the descriptor asks for a flag no element can have.
    Refused,
    /// Accepted, and it names a value to convert and store at this index.
    Stores(usize),
    /// Accepted, and it names no value, so there is nothing to write.
    Accepted,
}

pub(super) fn indexed_definition_key(
    view: &TypedArrayData,
    key: PropertyKeyRef<'_>,
    descriptor: &Descriptor,
) -> Option<IndexedDefinition> {
    let index = canonical_numeric_index_key(key)?;
    let Some(at) = valid_integer_index(view, index) else {
        return Some(IndexedDefinition::Refused);
    };
    // Every flag of an integer-indexed property is fixed by the specification rather than
    // stored, so a descriptor that asks for a different one is refused whatever the view holds.
    if descriptor.is_accessor()
        || descriptor.configurable == Some(false)
        || descriptor.enumerable == Some(false)
        || descriptor.writable == Some(false)
    {
        return Some(IndexedDefinition::Refused);
    }
    Some(match descriptor.value {
        Some(_) => IndexedDefinition::Stores(at),
        None => IndexedDefinition::Accepted,
    })
}

/// The built-in a callable's prototype chain ends at, if it ends at one.
///
/// `class P extends Promise {}` links the two prototypes *and* the two constructors, and the
/// second link is what makes `P.resolve` mean `Promise.resolve`. It is an ordinary prototype
/// pointer, but the value it points at is an intrinsic rather than an object with a store, so
/// the walk over property stores has nowhere left to go and has to ask the built-in instead.
///
/// Bounded like every other prototype walk here: the chain is the program's business, running
/// out of Rust stack is not.
pub(super) fn inherited_builtin(closure: &Rc<Closure>) -> Option<Native> {
    let mut current = closure.properties.borrow().prototype().cloned();
    if current.is_none() {
        if let Some(prototype) = function_kind_prototype(closure) {
            return Some(prototype);
        }
    }
    for _ in 0..MAX_PROTOTYPE_DEPTH {
        current = match current {
            Some(Value::Native(native, _)) => return Some(native),
            Some(Value::Function(parent)) => {
                let prototype = parent.properties.borrow().prototype().cloned();
                match prototype {
                    Some(prototype) => Some(prototype),
                    // An ordinary function's implicit [[Prototype]] is virtual: it is not
                    // stored in ObjectData. When a derived class constructor inherits through
                    // another function, the walk must cross that function's implicit tail too.
                    // Otherwise `class B extends A {}` reaches A's store, stops before
                    // Function.prototype, and loses inherited `bind`/`call` methods.
                    None => Some(Value::Native(
                        function_kind_prototype(&parent)
                            .unwrap_or(Native::PrototypeOf(PrototypeTable::Function)),
                        value_realm(&Value::Function(parent)),
                    )),
                }
            }
            Some(Value::Object(object)) => object.borrow().prototype().cloned(),
            _ => return None,
        };
    }
    None
}

pub(super) fn define_own_property_key(
    intrinsics: &Intrinsics,
    target: &Value,
    key: PropertyKeyRef<'_>,
    descriptor: &Descriptor,
) -> Result<bool, ExecutionError> {
    // Module exports report writable descriptors because their live bindings can change,
    // but [[DefineOwnProperty]] may only acknowledge the current value and flags. The shared
    // own-property state resolver reads the cell for this key, not its link-time snapshot.
    if let Value::Object(object) = target
        && object.borrow().is_module_namespace()
        && !key.is_symbol()
    {
        let Some((Property::Value(current), _)) = own_property_state_key(intrinsics, target, key)
        else {
            return Ok(false);
        };
        if matches!(current, Value::Native(Native::Uninitialized, _)) {
            return Err(uninitialized_export_error(&key.owned().to_lossy_string()));
        }
        return Ok(descriptor.configurable != Some(true)
            && descriptor.enumerable != Some(false)
            && descriptor.writable != Some(false)
            && !descriptor.is_accessor()
            && descriptor
                .value
                .as_ref()
                .is_none_or(|wanted| same_value(wanted, &current)));
    }
    if let Value::Shared(facade) = target {
        let Some((Property::Value(current), attributes)) = facade.own_property(key) else {
            return Ok(false);
        };
        return Ok(descriptor.configurable != Some(true)
            && descriptor
                .enumerable
                .is_none_or(|wanted| wanted == attributes.enumerable)
            && !descriptor.is_accessor()
            && descriptor.writable != Some(true)
            && descriptor
                .value
                .as_ref()
                .is_none_or(|wanted| same_value(wanted, &current)));
    }
    // An array's `length` has a definition of its own, and it has to run *instead of* the
    // ordinary one: the ordinary one has no way to raise a `RangeError`, and no reason to
    // shorten the elements when the length it was handed is smaller than the array.
    if let Value::Array(items) = target
        && key.matches_ascii("length")
    {
        return array_set_length(items, descriptor);
    }
    // An integer-indexed exotic object answers `[[DefineOwnProperty]]` itself for every element
    // key, and it too has to run *instead of* the ordinary one. The conversion the definition
    // ends in is the program's own method, so [`Vm::define_own_key`] is the spelling every machine
    // caller uses; this one writes what it was handed, which is the same answer for every value
    // that is already a number.
    if let Value::TypedArray(view) = target
        && let Some(outcome) = indexed_definition_key(view, key, descriptor)
    {
        return Ok(match outcome {
            IndexedDefinition::Refused => false,
            IndexedDefinition::Accepted => true,
            IndexedDefinition::Stores(at) => {
                if let Some(value) = &descriptor.value {
                    view.set(at, value)?;
                }
                true
            }
        });
    }
    if matches!(
        target,
        Value::Native(Native::PrototypeOf(PrototypeTable::Array), _)
    ) && key.matches_ascii("length")
    {
        if let Some(value) = &descriptor.value {
            let number = value.as_number();
            if !number.is_finite()
                || number < 0.0
                || number.fract() != 0.0
                || number > MAX_ARRAY_LENGTH
            {
                return Err(ExecutionError::InvalidArrayLength);
            }
        }
    }
    let Some((current, attributes)) = own_property_state_key(intrinsics, target, key) else {
        if !is_extensible_target(intrinsics, target) {
            return Ok(false);
        }
        // A field left out of a descriptor that *creates* a property means `false`, which is the
        // one place the language's default is not `true`.
        let attributes = Attributes {
            writable: descriptor.writable.unwrap_or(false),
            enumerable: descriptor.enumerable.unwrap_or(false),
            configurable: descriptor.configurable.unwrap_or(false),
        };
        install_property_key(
            intrinsics,
            target,
            key,
            descriptor_property(descriptor, None),
            attributes,
        )?;
        if matches!(
            target,
            Value::Native(Native::PrototypeOf(PrototypeTable::Array), _)
        ) && let Some(index) = array_index_key(key)
        {
            let cur_len =
                match own_property_state_key(intrinsics, target, PropertyKeyRef::Utf8("length")) {
                    Some((Property::Value(v), _)) => v.as_number(),
                    _ => 0.0,
                };
            if (index as f64) >= cur_len {
                install_property_key(
                    intrinsics,
                    target,
                    PropertyKeyRef::Utf8("length"),
                    Property::Value(Value::Number((index + 1) as f64)),
                    Attributes {
                        writable: true,
                        enumerable: false,
                        configurable: false,
                    },
                )?;
            }
        }
        return Ok(true);
    };
    let current_is_accessor = matches!(current, Property::Accessor { .. });
    // A non-configurable property is a promise to whoever read its descriptor. Everything below
    // is that promise: the flags may not loosen, the kind may not change, and the one thing that
    // may still move is the value of a property that was left writable.
    if !attributes.configurable {
        if descriptor.configurable == Some(true) {
            return Ok(false);
        }
        if descriptor
            .enumerable
            .is_some_and(|wanted| wanted != attributes.enumerable)
        {
            return Ok(false);
        }
        if !descriptor.is_generic() && descriptor.is_accessor() != current_is_accessor {
            return Ok(false);
        }
        match &current {
            Property::Accessor { get, set } => {
                let held = |half: &Option<Value>| half.clone().unwrap_or(Value::Undefined);
                if descriptor
                    .get
                    .as_ref()
                    .is_some_and(|wanted| !same_value(wanted, &held(get)))
                {
                    return Ok(false);
                }
                if descriptor
                    .set
                    .as_ref()
                    .is_some_and(|wanted| !same_value(wanted, &held(set)))
                {
                    return Ok(false);
                }
            }
            Property::Value(held) if !attributes.writable => {
                if descriptor.writable == Some(true) {
                    return Ok(false);
                }
                if descriptor
                    .value
                    .as_ref()
                    .is_some_and(|wanted| !same_value(wanted, held))
                {
                    return Ok(false);
                }
            }
            Property::Value(_) => {}
        }
    }
    let writable = if descriptor.is_accessor() {
        false
    } else if current_is_accessor {
        // Turning an accessor into a data property starts from the defaults: there is no
        // writable flag on an accessor to carry over.
        descriptor.writable.unwrap_or(false)
    } else {
        descriptor.writable.unwrap_or(attributes.writable)
    };
    let merged = Attributes {
        writable,
        enumerable: descriptor.enumerable.unwrap_or(attributes.enumerable),
        configurable: descriptor.configurable.unwrap_or(attributes.configurable),
    };
    install_property_key(
        intrinsics,
        target,
        key,
        descriptor_property(descriptor, Some(current)),
        merged,
    )?;
    if matches!(
        target,
        Value::Native(Native::PrototypeOf(PrototypeTable::Array), _)
    ) && let Some(index) = array_index_key(key)
    {
        let cur_len =
            match own_property_state_key(intrinsics, target, PropertyKeyRef::Utf8("length")) {
                Some((Property::Value(v), _)) => v.as_number(),
                _ => 0.0,
            };
        if (index as f64) >= cur_len {
            install_property_key(
                intrinsics,
                target,
                PropertyKeyRef::Utf8("length"),
                Property::Value(Value::Number((index + 1) as f64)),
                Attributes {
                    writable: true,
                    enumerable: false,
                    configurable: false,
                },
            )?;
        }
    }
    Ok(true)
}
