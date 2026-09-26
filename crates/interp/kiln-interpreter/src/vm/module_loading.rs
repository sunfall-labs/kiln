use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// Coerce what `import()` was given into a specifier string.
    ///
    /// `Ok(Err(value))` is a rejection rather than a throw: everything after the argument has
    /// been evaluated settles the promise, so a `toString` that throws hands its value back here
    /// instead of unwinding. The host handler is what makes that possible, exactly as it does
    /// for a promise reaction that throws.
    pub(super) fn import_specifier(
        &mut self,
        specifier: &Value,
        instruction: usize,
    ) -> Result<Result<String, Value>, ExecutionError> {
        if matches!(specifier, Value::Symbol(_)) {
            // A symbol has no string form, and the resulting TypeError rejects like any other
            // failure of the coercion.
            let error = self.construct_error(
                ErrorKind::Type,
                &[Value::String(
                    "Cannot convert a Symbol value to a string"
                        .to_owned()
                        .into(),
                )],
            );
            return Ok(Err(error));
        }
        let installed = self.handlers.len();
        self.handlers.push(Handler {
            target: HOST_HANDLER,
            frame_depth: self.frames.len(),
            stack_height: self.stack.len(),
            native_depth: self.native_depth,
        });
        let outcome = self.as_primitive(specifier, PrimitiveHint::String, instruction);
        self.handlers.truncate(installed);
        match outcome {
            Ok(Value::Symbol(_)) => {
                let error = self.construct_error(
                    ErrorKind::Type,
                    &[Value::String(
                        "Cannot convert a Symbol value to a string"
                            .to_owned()
                            .into(),
                    )],
                );
                Ok(Err(error))
            }
            Ok(primitive) => Ok(Ok(primitive.to_display_string())),
            Err(ExecutionError::CaughtByHost) => {
                Ok(Err(self.caught.take().unwrap_or(Value::Undefined)))
            }
            Err(error) => Err(error),
        }
    }

    /// Read dynamic import attributes through the VM's ordinary property machinery. This keeps
    /// getters, proxies and inherited properties observable exactly once before the typed loader
    /// service is called.
    pub(super) fn normalize_import_attributes(
        &mut self,
        options: &Value,
        instruction: usize,
    ) -> Result<Result<Option<ImportAttributes>, Value>, ExecutionError> {
        let installed = self.handlers.len();
        self.handlers.push(Handler {
            target: HOST_HANDLER,
            frame_depth: self.frames.len(),
            stack_height: self.stack.len(),
            native_depth: self.native_depth,
        });
        let outcome = if !is_an_object(options) {
            Ok(Err(self.construct_error(
                ErrorKind::Type,
                &[Value::String("import options must be an object".into())],
            )))
        } else {
            self.read_property(options, "with", instruction)
                .and_then(|with| {
                    if matches!(with, Value::Undefined) {
                        return Ok(Ok(None));
                    }
                    if !is_an_object(&with) {
                        return Ok(Err(self.construct_error(
                            ErrorKind::Type,
                            &[Value::String("import attributes must be an object".into())],
                        )));
                    }
                    let mut type_value = None;
                    let mut entries = Vec::new();
                    for key in self.own_keys(&with, instruction)? {
                        let Some((_, attributes)) =
                            self.own_state(&with, &Value::String(key.clone().into()), instruction)?
                        else {
                            continue;
                        };
                        if !attributes.enumerable {
                            continue;
                        }
                        let value = self.read_property_key(
                            &with,
                            PropertyKeyRef::Units(&key),
                            instruction,
                        )?;
                        entries.push((key, value));
                    }
                    for (key, value) in entries {
                        if key == "type" {
                            type_value = Some(value);
                        } else {
                            return Ok(Err(self.construct_error(
                                ErrorKind::Type,
                                &[Value::String("unsupported import attribute".into())],
                            )));
                        }
                    }
                    let Some(value) = type_value else {
                        return Ok(Ok(Some(ImportAttributes { type_: None })));
                    };
                    match value {
                        Value::String(value) if value == "file" => Ok(Ok(Some(ImportAttributes {
                            type_: Some(value.to_lossy_string()),
                        }))),
                        Value::String(_) => Ok(Err(self.construct_error(
                            ErrorKind::Type,
                            &[Value::String("unsupported import attribute `type`".into())],
                        ))),
                        _ => Ok(Err(self.construct_error(
                            ErrorKind::Type,
                            &[Value::String(
                                "import attribute values must be strings".into(),
                            )],
                        ))),
                    }
                })
        };
        self.handlers.truncate(installed);
        match outcome {
            Ok(Ok(attributes)) => Ok(Ok(attributes)),
            Ok(Err(error)) => Ok(Err(error)),
            Err(ExecutionError::CaughtByHost) => {
                Ok(Err(self.caught.take().unwrap_or(Value::Undefined)))
            }
            Err(error) => Err(error),
        }
    }

    /// Resolve, evaluate and answer one `import(specifier)`.
    ///
    /// `Ok(Err(value))` is the rejection the promise gets: a specifier that names nothing, a
    /// module that is not a module, a host that cannot load the module, or a module whose body threw.
    pub(super) fn import_module(
        &mut self,
        specifier: &str,
        options: Option<&ImportAttributes>,
    ) -> Result<Result<Value, Value>, ExecutionError> {
        let Some(loader) = self.module_loader.as_ref() else {
            let message = if options.is_some() {
                "no option-aware module loader is installed"
            } else {
                "no module loader is installed"
            };
            return Ok(Err(self.construct_error(
                ErrorKind::Type,
                &[Value::String(message.to_owned().into())],
            )));
        };
        let plan_result = loader.load(ModuleLoadRequest {
            importer: self.module_path.as_deref().map(str::to_owned),
            specifier: specifier.to_owned(),
            attributes: options.cloned(),
        });
        let plan = match plan_result {
            Ok(plan) => plan,
            Err(ModuleLoadError::Unsupported(message)) => {
                let error = self.construct_error(ErrorKind::Type, &[Value::String(message.into())]);
                return Ok(Err(error));
            }
            Err(ModuleLoadError::NotFound(message)) => {
                let error = self.construct_error(ErrorKind::Type, &[Value::String(message.into())]);
                return Ok(Err(error));
            }
            Err(ModuleLoadError::Syntax(message)) => {
                let error =
                    self.construct_error(ErrorKind::Syntax, &[Value::String(message.into())]);
                return Ok(Err(error));
            }
        };

        let realm = self.current_realm;
        if let Some(DynamicModuleState::Failed(failure)) =
            self.dynamic_modules.get(&(realm, plan.key.clone()))
        {
            return Ok(Err(failure.clone()));
        }
        if let Some(namespace) = self.dynamic_namespaces.get(&(realm, plan.key.clone())) {
            return Ok(Ok(namespace.clone()));
        }

        let mut evaluated: Vec<Rc<Vec<Value>>> = Vec::with_capacity(plan.modules.len());
        for planned in &plan.modules {
            if let Some(state) = self.dynamic_modules.get(&(realm, planned.key.clone())) {
                match state {
                    DynamicModuleState::Evaluated(cached) => {
                        evaluated.push(Rc::clone(cached));
                        continue;
                    }
                    DynamicModuleState::Failed(failure) => return Ok(Err(failure.clone())),
                }
            }
            let mut locals = vec![Value::Undefined; planned.program.local_count as usize];
            for (slot, binding) in &planned.bindings {
                let value = self.module_binding(binding, &evaluated)?;
                let Some(local) = locals.get_mut(*slot as usize) else {
                    return Err(ExecutionError::LocalOutOfBounds {
                        index: *slot as usize,
                    });
                };
                // An import slot always holds a cell, because an import is a live view of the
                // exporter's binding -- the same rule `execute_module` follows for a statically
                // linked graph.
                *local = match value {
                    Value::Cell(_) => value,
                    other => Value::Cell(Rc::new(RefCell::new(other))),
                };
            }
            let module_path = Rc::from(planned.key.as_str());
            match self.drive_module(Rc::clone(&planned.program), locals, Some(module_path))? {
                Ok(returned) => {
                    let Value::Array(exported) = returned else {
                        let error = self.construct_error(
                            ErrorKind::Type,
                            &[Value::String(
                                format!("module `{}` did not hand back its exports", planned.key)
                                    .into(),
                            )],
                        );
                        self.dynamic_modules.insert(
                            (realm, planned.key.clone()),
                            DynamicModuleState::Failed(error.clone()),
                        );
                        return Ok(Err(error));
                    };
                    let exported = Rc::new(exported.items.borrow().clone());
                    self.dynamic_modules.insert(
                        (realm, planned.key.clone()),
                        DynamicModuleState::Evaluated(Rc::clone(&exported)),
                    );
                    evaluated.push(exported);
                }
                Err(thrown) => {
                    self.dynamic_modules.insert(
                        (realm, planned.key.clone()),
                        DynamicModuleState::Failed(thrown.clone()),
                    );
                    return Ok(Err(thrown));
                }
            }
        }

        let mut entries = Vec::with_capacity(plan.namespace.len());
        for (name, reference) in &plan.namespace {
            entries.push((name.clone(), Self::planned_value(reference, &evaluated)?));
        }
        let namespace = namespace_object(entries);
        self.dynamic_namespaces
            .insert((realm, plan.key), namespace.clone());
        Ok(Ok(namespace))
    }

    /// Build one value a planned module imports out of what the plan has already evaluated.
    pub(super) fn module_binding(
        &mut self,
        binding: &ModuleBinding,
        evaluated: &[Rc<Vec<Value>>],
    ) -> Result<Value, ExecutionError> {
        match binding {
            ModuleBinding::Ready(value) => Ok(value.clone()),
            ModuleBinding::Value(reference) => Self::planned_value(reference, evaluated),
            ModuleBinding::Namespace(entries) => {
                let mut names = Vec::with_capacity(entries.len());
                for (name, reference) in entries {
                    names.push((name.clone(), Self::planned_value(reference, evaluated)?));
                }
                Ok(namespace_object(names))
            }
        }
    }

    pub(super) fn planned_value(
        reference: &ModuleValueRef,
        evaluated: &[Rc<Vec<Value>>],
    ) -> Result<Value, ExecutionError> {
        evaluated
            .get(reference.module)
            .and_then(|values| values.get(reference.index))
            .cloned()
            .ok_or_else(|| ExecutionError::DynamicImportUnsupported {
                message: "a module plan referred to a value that was never evaluated".to_owned(),
            })
    }

    /// Run one planned module body to completion and hand back what it exported.
    ///
    /// A module is a program, not a function, so it enters a frame of its own and `global_base`
    /// moves with it -- the same arrangement `enter_eval` makes, and for the same reason: its
    /// top-level slots are what its nested functions call module scope.
    pub(super) fn drive_module(
        &mut self,
        program: Rc<Program>,
        locals: Vec<Value>,
        module_path: Option<Rc<str>>,
    ) -> Result<Result<Value, Value>, ExecutionError> {
        if self.native_depth >= self.limits.max_native_depth {
            return Err(ExecutionError::CallDepthExceeded {
                limit: self.limits.max_native_depth,
            });
        }
        if self.frames.len() >= self.limits.max_call_depth {
            return Err(ExecutionError::CallDepthExceeded {
                limit: self.limits.max_call_depth,
            });
        }
        self.native_depth += 1;
        let installed = self.handlers.len();
        self.handlers.push(Handler {
            target: HOST_HANDLER,
            frame_depth: self.frames.len(),
            stack_height: self.stack.len(),
            native_depth: self.native_depth,
        });
        let depth = self.frames.len();
        let strictness = Rc::new(Strictness::always(&program));
        self.frames.push(Frame {
            realm: self.current_realm,
            program: std::mem::replace(&mut self.program, program),
            strictness: std::mem::replace(&mut self.strictness, strictness),
            strict: std::mem::replace(&mut self.strict, true),
            module_path: self.module_path.clone(),
            function: self.function.take(),
            // A module body is always strict, so its `this` is `undefined` rather than the
            // global object.
            this_value: std::mem::replace(&mut self.this_value, Value::Undefined),
            new_target: std::mem::replace(&mut self.new_target, Value::Undefined),
            arguments: std::mem::take(&mut self.arguments),
            arguments_object: self.arguments_object.take(),
            mapped_arguments: std::mem::replace(&mut self.mapped_arguments, false),
            constructing: None,
            locals: std::mem::replace(&mut self.locals, locals),
            global_base: self.global_base,
            instruction: self.instruction,
            stack_base: self.stack.len(),
        });
        self.module_path = module_path;
        self.global_base = self.frames.len();
        self.instruction = 0;
        let outcome = self.run_frames(depth).map(|result| result.value);
        self.handlers.truncate(installed);
        self.native_depth -= 1;
        match outcome {
            Ok(value) => Ok(Ok(value)),
            Err(ExecutionError::CaughtByHost) => {
                Ok(Err(self.caught.take().unwrap_or(Value::Undefined)))
            }
            Err(error) => Err(error),
        }
    }
}
