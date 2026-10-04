use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// The object a `new` call starts from: an empty object whose prototype is the function's.
    ///
    /// The prototype object is created on first use rather than with every closure, so an
    /// ordinary function never used as a constructor never allocates one.
    pub(super) fn instantiate(&mut self, closure: &Rc<Closure>) -> Value {
        let prototype = prototype_object(closure);
        // `F.prototype = undefined` (or any primitive) is substituted with `Object.prototype`, as
        // `instantiate_for` below does; linking the primitive left `new F()` inheriting nothing.
        let prototype = if is_reflect_target(&prototype) {
            prototype
        } else {
            Value::Native(
                Native::PrototypeOf(PrototypeTable::Object),
                RealmId::PRIMARY,
            )
        };
        let mut instance = ObjectData::default();
        instance.set_prototype(prototype);
        Value::Object(Rc::new(RefCell::new(instance)))
    }

    /// The same instance, with its prototype taken from `new.target` rather than from the
    /// function being run.
    ///
    /// The two coincide for `new C()`, which is why the fast path checks for that first: reading
    /// `prototype` off the target can run a program's getter, and doing it on every `new` would
    /// be a call nothing asked for. A `prototype` that is not an object falls back to
    /// `Object.prototype`, which is what the specification substitutes rather than failing.
    pub(super) fn instantiate_for(
        &mut self,
        closure: &Rc<Closure>,
        new_target: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if matches!(new_target, Value::Function(target) if Rc::ptr_eq(target, closure)) {
            return Ok(self.instantiate(closure));
        }
        let named = self.read_property(new_target, "prototype", instruction)?;
        let prototype = if is_reflect_target(&named) {
            named
        } else {
            Value::Native(
                Native::PrototypeOf(PrototypeTable::Object),
                RealmId::PRIMARY,
            )
        };
        let mut instance = ObjectData::default();
        instance.set_prototype(prototype);
        Ok(Value::Object(Rc::new(RefCell::new(instance))))
    }

    /// Build a closure, resolving each upvalue descriptor against the frame creating it.
    pub(super) fn make_closure(&mut self, function: u32) -> Result<Rc<Closure>, ExecutionError> {
        // What `arguments` means *here*, which for an arrow defined inside another arrow is what
        // that one captured -- not its own call's list, which is its rest parameter's business
        // and nothing else's. Taken before the proto below, because building it reads the frame
        // and only an arrow needs it at all.
        let arrow = self
            .program
            .functions
            .get(function as usize)
            .ok_or(ExecutionError::UnknownFunction { function })?
            .arrow;
        let captured_arguments = arrow.then(|| self.arguments_object());
        let Some(proto) = self.program.functions.get(function as usize) else {
            return Err(ExecutionError::UnknownFunction { function });
        };
        let mut upvalues = Vec::with_capacity(proto.upvalues.len());
        for descriptor in &proto.upvalues {
            let cell = match descriptor {
                Upvalue::ParentLocal(slot) | Upvalue::NamedParentLocal { slot, .. } => {
                    self.cell(*slot as usize)?
                }
                Upvalue::ParentUpvalue(index) | Upvalue::NamedParentUpvalue { index, .. } => {
                    let Some(current) = &self.function else {
                        // Only a nested function has a parent closure to capture from, and the
                        // compiler never emits ParentUpvalue at module level.
                        return Err(ExecutionError::MissingUpvalue { index: *index });
                    };
                    current
                        .upvalues
                        .get(*index as usize)
                        .cloned()
                        .ok_or(ExecutionError::MissingUpvalue { index: *index })?
                }
            };
            upvalues.push(cell);
        }
        let closure = Rc::new(Closure {
            function,
            // Read from the proto rather than from a table on the machine. A cached table can
            // only ever describe *one* program, and `self.program` is not one program: it is
            // whichever program the running frame belongs to, which changes on every eval and on
            // every call into a function from another module. Indexed by a number that means
            // something different in each of them, such a table answered `eval("(function f(){})")`
            // with the name of the caller's function number 0 -- and panicked outright when the
            // caller had fewer functions than the eval did.
            name: proto.name.as_deref().map(Rc::from),
            inferred_name: proto.inferred_name.clone(),
            upvalues,
            realm: self.current_realm,
            // An arrow takes `this` from where it is written; an ordinary function takes it from
            // how it is called, so it captures nothing here.
            captured_this: if proto.arrow {
                Some(self.this_value.clone())
            } else {
                None
            },
            program: Rc::clone(&self.program),
            strictness: Rc::clone(&self.strictness),
            module_path: self.module_path.clone(),
            module_generation: self.module_generation,
            captured_new_target: proto.arrow.then(|| self.new_target.clone()),
            captured_arguments,
            prototype: RefCell::new(None),
            properties: RefCell::new(ObjectData::default()),
        });
        {
            // `length` and `name` are stored rather than derived on read, because a program can
            // *remove* them: they are the one pair the specification marks configurable, and a
            // decorator that renames a function does exactly that. Derived, a `delete` would
            // have nothing to delete and the property would come straight back.
            let mut properties = closure.properties.borrow_mut();
            properties.define_value(
                "length",
                Value::Number(f64::from(proto.declared_length)),
                FUNCTION_IDENTITY,
            );
            properties.define_value(
                "name",
                Value::String(closure.display_name_units()),
                FUNCTION_IDENTITY,
            );
        }
        // Registered before it is handed out, so a closure that ends up in a cycle is still
        // something `release_cycles` can reach.
        if self.closures.len() >= self.closures_prune_at {
            self.closures.retain(|entry| entry.strong_count() > 0);
            self.closures_prune_at = self
                .closures
                .len()
                .saturating_add(self.closures.len().max(1024));
        }
        self.closures.push(Rc::downgrade(&closure));
        let module_map = self.gc_module_map();
        let mut module_map = module_map.borrow_mut();
        if module_map.gc_closures.len() >= module_map.gc_closures_prune_at {
            module_map
                .gc_closures
                .retain(|entry| entry.strong_count() > 0);
            module_map.gc_closures_prune_at = module_map
                .gc_closures
                .len()
                .saturating_add(module_map.gc_closures.len().max(1024));
        }
        module_map.gc_closures.push(Rc::downgrade(&closure));
        Ok(closure)
    }
}
