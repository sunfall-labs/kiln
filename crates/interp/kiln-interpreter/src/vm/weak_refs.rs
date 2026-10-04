use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// `CanBeHeldWeakly(value)`.
    ///
    /// An object, or a symbol that is not in the global registry. The registry is the whole
    /// reason symbols are not simply admitted: `Symbol.for("x")` is reachable from any code that
    /// knows the key, so it can never become unreachable, and holding one weakly would be a
    /// promise the engine could never keep.
    pub(super) fn can_be_held_weakly(&self, value: &Value) -> bool {
        match value {
            Value::Symbol(_) => match &self.module_realm {
                Some(realm) => !realm
                    .borrow()
                    .symbol_registry
                    .iter()
                    .any(|(_, registered)| strict_equal(registered, value)),
                None => !self
                    .symbol_registry
                    .iter()
                    .any(|(_, registered)| strict_equal(registered, value)),
            },
            other => is_reflect_target(other),
        }
    }

    /// `new WeakRef(target)`.
    pub(crate) fn construct_weak_ref(
        &mut self,
        arguments: &[Value],
        _current: usize,
    ) -> Result<Value, ExecutionError> {
        let target = arguments.first().cloned().unwrap_or(Value::Undefined);
        if !self.can_be_held_weakly(&target) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "WeakRef: target must be an object or an unregistered symbol".to_owned(),
            });
        }
        let mut data = ObjectData::default();
        if let Value::Object(object) = &target {
            data.weak_ref_object_target = Some(Rc::downgrade(object));
            let module_map = self.gc_module_map();
            let mut module_map = module_map.borrow_mut();
            if module_map.gc_weak_ref_objects.len() >= module_map.gc_weak_ref_objects_prune_at {
                module_map
                    .gc_weak_ref_objects
                    .retain(|entry| entry.strong_count() > 0);
                module_map.gc_weak_ref_objects_prune_at = module_map
                    .gc_weak_ref_objects
                    .len()
                    .saturating_add(module_map.gc_weak_ref_objects.len().max(1024));
            }
            module_map.gc_weak_ref_objects.push(Rc::downgrade(object));
            data.set_non_enumerable(WEAK_REF_TARGET, Value::Undefined);
        } else if let Value::Shared(facade) = &target {
            data.weak_shared_target = Some(Rc::downgrade(&facade));
            data.set_non_enumerable(WEAK_REF_TARGET, Value::Undefined);
        } else {
            data.set_non_enumerable(WEAK_REF_TARGET, target.clone());
        }
        // A SharedFacade owns the graph lease. Keeping the facade alive through this realm's
        // job would therefore make a WeakRef extend the lifetime of an otherwise dead graph.
        // The graph is immutable and its weak target can be upgraded directly at deref time.
        if !matches!(target, Value::Shared(_)) {
            self.gc_module_map()
                .borrow_mut()
                .weak_ref_keepalive
                .push(target);
        }
        data.set_prototype(Value::Native(
            Native::PrototypeOf(PrototypeTable::WeakRef),
            RealmId::PRIMARY,
        ));
        Ok(Value::Object(Rc::new(RefCell::new(data))))
    }

    /// `new FinalizationRegistry(cleanupCallback)`.
    pub(super) fn construct_finalization_registry(
        &mut self,
        arguments: &[Value],
        current: usize,
    ) -> Result<Value, ExecutionError> {
        let callback = arguments.first().cloned().unwrap_or(Value::Undefined);
        if !is_callable(&callback) {
            return Err(ExecutionError::NotCallable {
                instruction: current,
                value: callback.type_name(),
            });
        }
        let mut data = ObjectData::default();
        data.set_non_enumerable(REGISTRY_CALLBACK, callback);
        data.set_non_enumerable(REGISTRY_CELLS, Value::Undefined);
        data.set_prototype(Value::Native(
            Native::PrototypeOf(PrototypeTable::FinalizationRegistry),
            RealmId::PRIMARY,
        ));
        Ok(Value::Object(Rc::new(RefCell::new(data))))
    }

    /// The reserved slot of a `WeakRef` or a registry, or a `TypeError` naming the receiver.
    ///
    /// The brand check is the slot: these two types are ordinary objects with a hidden property,
    /// so "is this really one" and "where is its state" are the same question.
    pub(super) fn weak_slot(
        &self,
        receiver: &Value,
        slot: &str,
        what: &str,
    ) -> Result<Value, ExecutionError> {
        if let Value::Object(object) = receiver
            && let Some(found) = object.borrow().get_own(slot)
        {
            return Ok(found.clone());
        }
        Err(ExecutionError::TypeErrorMessage {
            message: format!("{what} method called on an incompatible receiver"),
        })
    }

    /// `WeakRef.prototype.deref()`.
    pub(crate) fn weak_ref_deref(
        &mut self,
        receiver: &Value,
        _instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let stored = self.weak_slot(receiver, WEAK_REF_TARGET, "WeakRef.prototype.deref")?;
        if let Value::Object(object) = receiver {
            let data = object.borrow();
            if let Some(target) = &data.weak_ref_object_target {
                if let Some(target) = target.upgrade() {
                    let value = Value::Object(target);
                    self.gc_module_map()
                        .borrow_mut()
                        .weak_ref_keepalive
                        .push(value.clone());
                    return Ok(value);
                }
                return Ok(Value::Undefined);
            }
            if let Some(target) = &data.weak_shared_target {
                if let Some(target) = target.upgrade() {
                    let value = Value::Shared(target);
                    return Ok(value);
                }
                return Ok(Value::Undefined);
            }
        }
        self.gc_module_map()
            .borrow_mut()
            .weak_ref_keepalive
            .push(stored.clone());
        Ok(stored)
    }

    /// `FinalizationRegistry.prototype.register(target, heldValue, unregisterToken)`.
    pub(super) fn finalization_registry_register(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        _instruction: usize,
    ) -> Result<Value, ExecutionError> {
        self.weak_slot(
            receiver,
            REGISTRY_CELLS,
            "FinalizationRegistry.prototype.register",
        )?;
        let target = arguments.first().cloned().unwrap_or(Value::Undefined);
        if !self.can_be_held_weakly(&target) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "FinalizationRegistry.prototype.register: target must be an object or an unregistered symbol"
                    .to_owned(),
            });
        }
        let held = arguments.get(1).cloned().unwrap_or(Value::Undefined);
        // A target that is also its own held value would keep itself alive through the registry
        // for as long as the registry lived, which is exactly the leak the registry exists to
        // avoid -- so the specification refuses the pair outright rather than leaking quietly.
        if same_value(&target, &held) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "FinalizationRegistry.prototype.register: target and holdings must not be the same"
                    .to_owned(),
            });
        }
        let token = arguments.get(2).cloned().unwrap_or(Value::Undefined);
        if !matches!(token, Value::Undefined) && !self.can_be_held_weakly(&token) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "FinalizationRegistry.prototype.register: unregisterToken must be an object or an unregistered symbol"
                    .to_owned(),
            });
        }
        if let Value::Object(object) = receiver {
            object
                .borrow_mut()
                .finalization_cells
                .push(FinalizationCell {
                    target: WeakCellValue::new(target),
                    held,
                    token: (!matches!(token, Value::Undefined)).then(|| WeakCellValue::new(token)),
                });
        }
        Ok(Value::Undefined)
    }

    /// `FinalizationRegistry.prototype.unregister(unregisterToken)`.
    ///
    /// Answers only whether anything was removed, and matches on the *token* alone: a cell
    /// registered without one can never be removed, which is why a program that means to
    /// unregister has to supply a token it kept.
    pub(super) fn finalization_registry_unregister(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        _instruction: usize,
    ) -> Result<Value, ExecutionError> {
        self.weak_slot(
            receiver,
            REGISTRY_CELLS,
            "FinalizationRegistry.prototype.unregister",
        )?;
        let token = arguments.first().cloned().unwrap_or(Value::Undefined);
        if !self.can_be_held_weakly(&token) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "FinalizationRegistry.prototype.unregister: unregisterToken must be an object or an unregistered symbol"
                    .to_owned(),
            });
        }
        let Value::Object(object) = receiver else {
            return Ok(Value::Boolean(false));
        };
        let mut object = object.borrow_mut();
        let before = object.finalization_cells.len();
        object.finalization_cells.retain(|cell| {
            !cell
                .token
                .as_ref()
                .and_then(WeakCellValue::upgrade)
                .is_some_and(|recorded| same_value(&recorded, &token))
        });
        Ok(Value::Boolean(object.finalization_cells.len() != before))
    }
}
