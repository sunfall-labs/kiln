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
        let generation = self.module_map.borrow().generation(self.current_realm);
        self.import_module_in_generation(specifier, options, generation)
    }

    pub(super) fn import_module_in_generation(
        &mut self,
        specifier: &str,
        options: Option<&ImportAttributes>,
        generation: u64,
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
        // A prior `require()` may have evaluated a host module without constructing its ESM
        // namespace. Namespace preparation is idempotent and must precede returning an already
        // settled host record, regardless of the dynamic import generation.
        for planned in &plan.modules {
            if matches!(planned.body, PlannedBody::Host)
                && self
                    .module_map
                    .borrow()
                    .status_in_generation(realm, generation, &planned.key)
                    .is_some_and(|status| matches!(status, ModuleStatus::Evaluated))
                && let Err(value) = self.prepare_host_namespace(&planned.key)?
            {
                return Ok(Err(value));
            }
        }
        // A module the realm has already settled is answered from its record: the same
        // namespace object every time, or the same error (ECMA-262 16.2.1.5.3 step 4).
        let settled = self
            .module_map
            .borrow()
            .status_in_generation(realm, generation, &plan.key);
        match settled {
            Some(ModuleStatus::Failed(failure)) => return Ok(Err(failure)),
            Some(ModuleStatus::Evaluated) => {
                return Ok(Ok(self
                    .module_map
                    .borrow_mut()
                    .namespace_in_generation(realm, generation, &plan.key)));
            }
            _ => {}
        }

        for planned in &plan.modules {
            match self
                .module_map
                .borrow()
                .status_in_generation(realm, generation, &planned.key)
            {
                // Evaluated by whoever reached it first -- the static graph, or another
                // `import()` -- or still evaluating further up this realm's stack (a module
                // that imports itself, or one suspended at a top-level `await`). Either way it
                // has its one evaluation, and this request links to it.
                Some(ModuleStatus::Evaluated | ModuleStatus::Evaluating) => continue,
                Some(ModuleStatus::Failed(failure)) => return Ok(Err(failure)),
                Some(ModuleStatus::Linked) | None => {}
            }
            if let Err(rejection) = self.evaluate_planned(planned, realm, generation)? {
                return Ok(Err(rejection));
            }
        }

        Ok(Ok(self
            .module_map
            .borrow_mut()
            .namespace_in_generation(realm, generation, &plan.key)))
    }

    /// Evaluate one planned module into the realm's module map.
    ///
    /// `Ok(Err(value))` is the module's failure, which is also recorded in the map so that every
    /// later import of it rejects with the same value.
    fn evaluate_planned(
        &mut self,
        planned: &PlannedModule,
        realm: RealmId,
        generation: u64,
    ) -> Result<Result<(), Value>, ExecutionError> {
        let (program, names, engine_private) = match &planned.body {
            PlannedBody::Exports(entries) => {
                let exports = entries.iter().cloned().collect();
                self.module_map.borrow_mut().publish_in_generation(
                    realm,
                    generation,
                    &planned.key,
                    exports,
                );
                return Ok(Ok(()));
            }
            PlannedBody::Host => {
                let mut result = self.evaluate_host_module(&planned.key)?;
                if matches!(&result, Ok(())) {
                    result = self.prepare_host_namespace(&planned.key)?;
                }
                // CommonJS and builtin modules keep the host cache's identity across runner
                // resets. Their generation-specific records reuse canonical export cells and
                // cached namespace-read results; each generation creates its own namespace view.
                self.module_map
                    .borrow_mut()
                    .alias_generation(realm, 0, generation, &planned.key);
                return Ok(result);
            }
            PlannedBody::Source {
                program,
                exports,
                engine_private,
            } => (program, exports, *engine_private),
        };
        let mut locals = vec![Value::Undefined; program.local_count as usize];
        for (slot, binding) in &planned.bindings {
            let value = match self.module_binding_in_generation(binding, realm, generation) {
                Some(value) => value,
                None => return Ok(Err(self.missing_binding(planned, binding))),
            };
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
        // An engine builtin sees the realm's engine-private object while its top-level code
        // runs, and only then. Whether kiln's extensions are installed is the module's own
        // question to answer (it names itself in the refusal), so both engines say it once.
        let private_window = if engine_private {
            match self.global_object_in(realm) {
                Value::Object(global) => Some(open_private_window(&mut global.borrow_mut())),
                _ => None,
            }
        } else {
            None
        };
        self.module_map
            .borrow_mut()
            .begin_in_generation(realm, generation, &planned.key);
        let module_result =
            self.drive_module(Rc::clone(program), locals, Some(module_path), generation);
        if let Some(window) = private_window
            && let Value::Object(global) = self.global_object_in(realm)
        {
            close_private_window(&mut global.borrow_mut(), window);
        }
        let returned = match module_result? {
            Ok(returned) => returned,
            Err(thrown) => {
                self.module_map.borrow_mut().fail_in_generation(
                    realm,
                    generation,
                    &planned.key,
                    thrown.clone(),
                );
                return Ok(Err(thrown));
            }
        };
        let Value::Array(returned) = returned else {
            let error = self.construct_error(
                ErrorKind::Type,
                &[Value::String(
                    format!("module `{}` did not hand back its exports", planned.key).into(),
                )],
            );
            self.module_map.borrow_mut().fail_in_generation(
                realm,
                generation,
                &planned.key,
                error.clone(),
            );
            return Ok(Err(error));
        };
        let mut exports: std::collections::HashMap<String, Value> = names
            .iter()
            .cloned()
            .zip(returned.items.borrow().iter().cloned())
            .collect();
        for (name, binding) in &planned.reexports {
            if exports.contains_key(name) {
                continue;
            }
            if let Some(value) = self.module_binding_in_generation(binding, realm, generation) {
                exports.insert(name.clone(), value);
            }
        }
        self.module_map.borrow_mut().publish_in_generation(
            realm,
            generation,
            &planned.key,
            exports,
        );
        Ok(Ok(()))
    }

    /// Evaluate a CommonJS or JSON module through the host's CommonJS loader, which publishes
    /// it into the map exactly as a `require` of it would have.
    fn evaluate_host_module(&mut self, key: &str) -> Result<Result<(), Value>, ExecutionError> {
        self.host_module_operation(key, false)
    }

    fn prepare_host_namespace(&mut self, key: &str) -> Result<Result<(), Value>, ExecutionError> {
        self.host_module_operation(key, true)
    }

    fn host_module_operation(
        &mut self,
        key: &str,
        namespace_only: bool,
    ) -> Result<Result<(), Value>, ExecutionError> {
        let instruction = self.instruction;
        let Some(loader) = self.commonjs_loader else {
            let error = self.construct_error(
                ErrorKind::Type,
                &[Value::String(
                    format!("`import()` of `{key}` is not implemented without a CommonJS loader")
                        .into(),
                )],
            );
            return Ok(Err(error));
        };
        // SAFETY: the loader outlives every machine it is installed in; see `commonjs_loader`.
        match unsafe {
            if namespace_only {
                (&mut *loader).prepare_module_namespace(key, instruction)
            } else {
                (&mut *loader).evaluate_module(key, instruction)
            }
        } {
            Ok(()) => Ok(Ok(())),
            Err(ExecutionError::Uncaught { message, .. }) => {
                let value = unsafe { (&mut *loader).take_thrown_value() };
                Ok(Err(value.unwrap_or_else(|| {
                    self.construct_error(ErrorKind::Error, &[Value::String(message.into())])
                })))
            }
            Err(ExecutionError::ModuleNotFound { message, .. }) => Ok(Err(
                self.construct_error(ErrorKind::Error, &[Value::String(message.into())])
            )),
            Err(
                error @ (ExecutionError::DynamicImportUnsupported { .. }
                | ExecutionError::Host { .. }),
            ) => Ok(Err(self.construct_error(
                ErrorKind::Type,
                &[Value::String(error.to_string().into())],
            ))),
            Err(error) => Err(error),
        }
    }

    /// The `SyntaxError` a plan earns by naming an export its module turned out not to have.
    fn missing_binding(&mut self, planned: &PlannedModule, binding: &ModuleBinding) -> Value {
        let name = match binding {
            ModuleBinding::Export { name, .. } => name.as_str(),
            _ => "*",
        };
        self.construct_error(
            ErrorKind::Syntax,
            &[Value::String(
                format!(
                    "module `{}` imports `{name}`, which is not exported",
                    planned.key
                )
                .into(),
            )],
        )
    }

    /// Build one value a planned module imports, from the realm's module map.
    ///
    /// A module that has finished hands over its binding; one that has not (a cycle, or a
    /// module still evaluating further up the stack) hands over a dead-zone cell its
    /// publication fills.
    fn module_binding_in_generation(
        &mut self,
        binding: &ModuleBinding,
        realm: RealmId,
        generation: u64,
    ) -> Option<Value> {
        match binding {
            ModuleBinding::Ready(value) => Some(value.clone()),
            ModuleBinding::Export { module, name } => self
                .module_map
                .borrow_mut()
                .binding_in_generation(realm, generation, module, name),
            ModuleBinding::Namespace { module } => Some(
                self.module_map
                    .borrow_mut()
                    .namespace_in_generation(realm, generation, module),
            ),
        }
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
        module_generation: u64,
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
            module_generation: self.module_generation,
            function: self.function.take(),
            method_name: std::mem::replace(&mut self.method_name, None),
            // A module body is always strict, so its `this` is `undefined` rather than the
            // global object.
            this_value: std::mem::replace(&mut self.this_value, Value::Undefined),
            new_target: std::mem::replace(&mut self.new_target, Value::Undefined),
            arguments: std::mem::take(&mut self.arguments),
            arguments_object: self.arguments_object.take(),
            mapped_arguments: std::mem::replace(&mut self.mapped_arguments, false),
            constructing: None,
            super_call: false,
            locals: std::mem::replace(&mut self.locals, locals),
            global_base: self.global_base,
            instruction: self.instruction,
            stack_base: self.stack.len(),
        });
        self.module_path = module_path;
        self.module_generation = module_generation;
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
