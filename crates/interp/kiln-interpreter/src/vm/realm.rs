use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    pub(crate) fn global_object(&mut self) -> Value {
        self.global_object_in(self.current_realm)
    }

    /// Make a realm and answer with its global object.
    ///
    /// A realm here is exactly three things: a global object, a set of error prototypes, and its
    /// share of the built-in mutation store -- the three `Vm` fields whose comments already said
    /// they were per-realm. Everything else the new realm has is the execution's and is shared
    /// deliberately: the clock, the entropy stream, the timezone and the locale are the
    /// capabilities ADR 0031 gives the *execution*, and a second realm that quietly reached past
    /// them for the host's would undo ADR 0015 in the one place nobody would look. The global
    /// symbol registry is shared because the specification shares it: `Symbol.for("x")` is the
    /// same symbol in every realm of an agent, which is the whole of what makes it a registry.
    ///
    /// The realm's clock therefore starts where this one is and runs on the same virtual time,
    /// which is what ADR 0015 asks for -- one clock, not the host's, and not a second one that
    /// could disagree with it.
    pub(crate) fn create_realm(&mut self) -> Result<Value, ExecutionError> {
        let realm = self.allocate_realm()?;
        self.realize_realm(realm)
    }

    /// Extend the machine's realm tables by one and answer the new realm's id.
    ///
    /// Split out from [`Vm::create_realm`] because a `ShadowRealm` wants exactly this and
    /// deliberately not the rest of it: `$262.createRealm` hands the *global object* back, which
    /// is the whole capability it grants, and a shadow realm must hand back nothing at all. The
    /// realm's global is still built on first use, by the code running inside it.
    pub(crate) fn allocate_realm(&mut self) -> Result<RealmId, ExecutionError> {
        let realm = RealmId(u32::try_from(self.global_objects.len()).map_err(|_| {
            ExecutionError::TypeErrorMessage {
                message: "too many realms".to_owned(),
            }
        })?);
        if let Some(shared) = &self.module_realm {
            let mut shared = shared.borrow_mut();
            shared.global_objects.push(None);
            shared
                .regexp_legacy_captures
                .push(std::array::from_fn(|_| StringUnits::default()));
        }
        self.global_objects.push(None);
        self.regexp_legacy_captures
            .push(std::array::from_fn(|_| StringUnits::default()));
        self.intrinsics.add_realm();
        Ok(realm)
    }

    /// Publish the captures from one successful RegExp execution. Module child VMs use the
    /// module session's realm-owned table so statics remain visible across module boundaries.
    pub(crate) fn publish_regexp_legacy_captures(
        &mut self,
        subject: &[u16],
        groups: &[Option<std::ops::Range<usize>>],
    ) {
        let mut captures: [StringUnits; 9] = std::array::from_fn(|_| StringUnits::default());
        for (capture, group) in captures.iter_mut().zip(groups.iter().skip(1)) {
            if let Some(range) = group {
                *capture = StringUnits::from_units(&subject[range.clone()]);
            }
        }
        let realm_index = self.current_realm.0 as usize;
        if let Some(shared) = &self.module_realm {
            if let Some(slot) = shared
                .borrow_mut()
                .regexp_legacy_captures
                .get_mut(realm_index)
            {
                *slot = captures;
            }
        } else if let Some(slot) = self.regexp_legacy_captures.get_mut(realm_index) {
            *slot = captures;
        }
    }

    pub(super) fn regexp_legacy_capture(&self, index: usize) -> Option<StringUnits> {
        if !(1..=9).contains(&index) {
            return None;
        }
        let realm_index = self.current_realm.0 as usize;
        if let Some(shared) = &self.module_realm {
            shared
                .borrow()
                .regexp_legacy_captures
                .get(realm_index)
                .and_then(|captures| captures.get(index - 1))
                .cloned()
        } else {
            self.regexp_legacy_captures
                .get(realm_index)
                .and_then(|captures| captures.get(index - 1))
                .cloned()
        }
    }

    /// `new ShadowRealm()`.
    ///
    /// The realm it allocates takes the execution's clock, entropy and locale exactly as
    /// `$262.createRealm`'s does, for the reason ADR 0031 gives: those are capabilities of the
    /// *execution*, and a realm that quietly reached past them for the host's would undo ADR
    /// 0015's virtual, epoch-start, UTC clock in the one place nobody would look. A shadow realm
    /// is a fresh set of intrinsics, not a fresh set of powers.
    pub(crate) fn construct_shadow_realm(&mut self) -> Result<Value, ExecutionError> {
        let realm = self.allocate_realm()?;
        // Built now rather than on first use, because building it runs the realm bootstrap,
        // which is program code and belongs here -- inside the constructor that asked for a
        // realm -- rather than inside whichever later read first touched its global object.
        self.realize_realm(realm)?;
        let mut instance = ObjectData {
            shadow_realm: Some(realm),
            ..ObjectData::default()
        };
        // `ShadowRealm.prototype` *of the realm doing the constructing*, which is what makes
        // `Reflect.construct(other.ShadowRealm, [])` produce an instance of `other`'s.
        instance.set_prototype(Value::Native(
            Native::PrototypeOf(PrototypeTable::ShadowRealm),
            self.current_realm,
        ));
        Ok(Value::Object(Rc::new(RefCell::new(instance))))
    }

    /// `ValidateShadowRealmObject`: the realm `receiver` evaluates in, or a `TypeError`.
    ///
    /// The check is for the internal slot and nothing else -- not the prototype, not a brand
    /// property -- which is why `evaluate.call({}, src)` throws instead of evaluating: a plain
    /// object has no slot, and there is no realm to reach for. A program cannot forge one,
    /// because nothing it can write puts a slot on an object.
    pub(crate) fn shadow_realm_of(&self, receiver: &Value) -> Result<RealmId, ExecutionError> {
        match receiver {
            Value::Object(object) => object.borrow().shadow_realm,
            _ => None,
        }
        .ok_or_else(|| ExecutionError::TypeErrorMessage {
            message: "ShadowRealm.prototype method called on an object that is not a ShadowRealm"
                .to_owned(),
        })
    }

    /// `GetWrappedValue(realm, value)`: what may cross the boundary into `realm`, and how.
    ///
    /// Three outcomes, and the specification allows no fourth: a primitive crosses as itself, a
    /// callable crosses as a *new* [`Value::Wrapped`] belonging to `realm`, and anything else
    /// does not cross at all. That last clause is the entire membrane. An object that got
    /// through would carry its own realm's prototype chain with it, and every identity question
    /// the receiving side then asked -- `instanceof`, `getPrototypeOf`, `constructor` -- would
    /// be answered about a realm it was never meant to reach. Refusing is not a limitation here;
    /// it is the feature.
    pub(crate) fn wrap_for_realm(
        &mut self,
        value: &Value,
        realm: RealmId,
    ) -> Result<Value, ExecutionError> {
        if !is_an_object(value) {
            return Ok(value.clone());
        }
        // `type_of` rather than `is_callable`, and the difference is a proxy: `is_callable`
        // answers `true` for every proxy without asking what it wraps, so a `new Proxy({}, {})`
        // -- an ordinary object by every rule the language has, `typeof` included -- would cross
        // the boundary as a callable and come back a wrapped function. That is exactly the leak
        // this check exists to stop, so the question asked here is the one `typeof` asks: it
        // follows a proxy to its target, and a proxy over a plain object is not callable.
        if value.type_of() != "function" {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!(
                    "cannot pass a non-callable object ({}) across a ShadowRealm boundary",
                    value.type_name()
                ),
            });
        }
        self.wrap_callable(value, realm)
    }

    /// `WrappedFunctionCreate` and `CopyNameAndLength`, which the specification runs together.
    ///
    /// `name` and `length` are read off the target *now*, once, and stored on the wrapper as
    /// ordinary data properties. Copied rather than forwarded for two reasons that pull the same
    /// way: a later write to the target's `name` must not be visible here, and a read must not
    /// re-enter the target realm to run a getter. Both reads are the program's own code -- either
    /// may be an accessor -- so each runs inside the target's realm and a throw from either
    /// becomes a `TypeError` belonging to `realm`, which is the caller's.
    pub(crate) fn wrap_callable(
        &mut self,
        target: &Value,
        realm: RealmId,
    ) -> Result<Value, ExecutionError> {
        let target_realm = value_realm(target);
        // `length` first and `name` only if it succeeded, which is the specification's order and
        // is observable: both may be accessors, and a `length` getter that throws must stop the
        // copy before a `name` getter runs at all.
        let copied = self.entering(target_realm, |vm| {
            Ok(match vm.catching(|vm| vm.wrapped_length(target)) {
                Ok(length) => vm
                    .catching(|vm| vm.wrapped_name(target))
                    .map(|name| (length, name)),
                Err(thrown) => Err(thrown),
            })
        })?;
        let Ok((length, name)) = copied else {
            // Deliberately discarding what was thrown: it is an object from the target realm,
            // and handing it out is exactly the leak this boundary exists to stop. The caller
            // learns that the copy failed, which is all the specification says it learns.
            return Err(ExecutionError::TypeErrorMessage {
                message: "reading name or length of a ShadowRealm function threw".to_owned(),
            });
        };
        let mut properties = ObjectData::default();
        // `Function.prototype` of the realm the wrapper belongs to -- the caller's -- so a
        // wrapped function is one of *its* functions in every way a program can check.
        properties.set_prototype(Value::Native(
            Native::PrototypeOf(PrototypeTable::Function),
            realm,
        ));
        // `{ writable: false, enumerable: false, configurable: true }` on both, which is the
        // shape every function's own `name` and `length` have.
        let own = Attributes {
            writable: false,
            enumerable: false,
            configurable: true,
        };
        properties.define_value("length", Value::Number(length), own);
        properties.define_value("name", Value::String(name.into()), own);
        Ok(Value::Wrapped(Rc::new(WrappedFunction {
            target: target.clone(),
            realm,
            properties: RefCell::new(properties),
        })))
    }

    /// The `length` a wrapper copies: the target's, when it has a numeric one, and 0 otherwise.
    ///
    /// `-Infinity` and every negative value clamp to 0 while `+Infinity` is kept, which is
    /// `ToIntegerOrInfinity` followed by the specification's own `max(_, 0)`. A target with no
    /// own `length` at all contributes 0 rather than inheriting `Function.prototype.length`,
    /// because the specification asks `HasOwnProperty` first.
    pub(crate) fn wrapped_length(&mut self, target: &Value) -> Result<f64, ExecutionError> {
        let present = match target {
            // A proxy answers its own-property questions through a trap, and the trap is the
            // program's code: reading a store here would say "no `length`" about every proxy
            // and copy 0 onto the wrapper.
            Value::Proxy(_) => {
                let described =
                    self.proxy_descriptor(target, &Value::String("length".to_owned().into()), 0)?;
                !matches!(described, Value::Undefined | Value::Null)
            }
            _ => own_attributes(&self.intrinsics, target, "length").is_some(),
        };
        if !present {
            return Ok(0.0);
        }
        let length = self.read_property(target, "length", 0)?;
        let Value::Number(number) = length else {
            return Ok(0.0);
        };
        if number.is_nan() {
            return Ok(0.0);
        }
        Ok(number.trunc().max(0.0))
    }

    /// `ShadowRealm.prototype.evaluate(sourceText)`.
    ///
    /// The caller realm is the running one: `call_value_inner` entered the realm of the
    /// `evaluate` function itself before this was reached, which is what makes
    /// `Other.prototype.evaluate.call(realm, src)` report `Other`'s errors and hand back a
    /// wrapper belonging to `Other` -- the specification's "current Realm Record", arrived at
    /// without this having to know anything about how it was called.
    pub(crate) fn shadow_realm_evaluate(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
    ) -> Result<Value, ExecutionError> {
        let caller = self.current_realm;
        let evaluated = self.shadow_realm_of(receiver)?;
        // A string, and not something that *converts* to one: `evaluate(1)` is a TypeError, and
        // an object with a `toString` is never called. The order matters too -- the receiver is
        // validated first, so `evaluate.call({}, 1)` reports the receiver.
        let Some(Value::String(source)) = arguments.first() else {
            return Err(ExecutionError::TypeErrorMessage {
                message: "ShadowRealm.prototype.evaluate expects a string".to_owned(),
            });
        };
        let source = source.clone();
        let produced = self.shadow_realm_run_units(&source, evaluated)?;
        match produced {
            Ok(value) => self.wrap_for_realm(&value, caller),
            // Every abrupt completion becomes *this* realm's TypeError, whatever it was: a
            // thrown primitive, a thrown Error, a ReferenceError from the shadow realm's own
            // code. The specification collapses them on purpose -- the alternative hands the
            // caller an object belonging to a realm it must not be able to reach -- so the
            // information genuinely is discarded rather than merely unreported.
            Err(()) => Err(ExecutionError::TypeErrorMessage {
                message: "ShadowRealm evaluation threw".to_owned(),
            }),
        }
    }

    /// `ShadowRealm.prototype.importValue(specifier, exportName)`.
    ///
    /// Both arguments are checked *before* the promise exists, and in the specification's order,
    /// which is observable: the specifier is coerced with `ToString` -- so a `toString` that
    /// throws throws here -- and `exportName` is then required to already be a string rather
    /// than coerced at all. A test that hands a throwing `toString` as the export name and
    /// asserts it was never called is asserting exactly this order.
    pub(crate) fn shadow_realm_import_value(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let caller = self.current_realm;
        let evaluated = self.shadow_realm_of(receiver)?;
        let specifier = arguments.first().cloned().unwrap_or(Value::Undefined);
        let specifier = self.as_string(&specifier, instruction)?;
        let Some(Value::String(export)) = arguments.get(1) else {
            return Err(ExecutionError::TypeErrorMessage {
                message: "ShadowRealm.prototype.importValue expects a string export name"
                    .to_owned(),
            });
        };
        let export = export.clone();
        // Queued rather than run, for the reason `import()` is queued: the specification loads
        // asynchronously even when the module is already in memory, and running it inline would
        // move the module's side effects ahead of the statement after the call.
        let promise = PromiseState::pending();
        let scheduled = self.scheduled(Job::ShadowImport {
            promise: Rc::clone(&promise),
            specifier,
            export: export.to_lossy_string(),
            evaluated,
            caller,
        });
        self.realm.queue_microtask(scheduled);
        Ok(Value::Promise(promise))
    }

    /// Load `specifier` in `evaluated`, and answer with its `export` wrapped into `caller`.
    ///
    /// Every failure is the same rejection: a specifier that names nothing, a module that will
    /// not parse, a module that throws while evaluating, an export that is not there, and a
    /// value that may not cross the boundary. They are collapsed for the reason `evaluate`
    /// collapses its failures -- the alternative rejects with an object belonging to a realm the
    /// caller must not reach -- so the rejection is always a `TypeError` built in `caller`.
    pub(crate) fn shadow_import(
        &mut self,
        specifier: &str,
        export: &str,
        evaluated: RealmId,
        caller: RealmId,
    ) -> Result<Settled, ExecutionError> {
        let imported = self.entering(evaluated, |vm| vm.import_module(specifier, None));
        let namespace = match imported {
            Ok(Ok(namespace)) => namespace,
            // Both the module's own failure and any error `entering` turned into a thrown value.
            Ok(Err(_)) | Err(ExecutionError::ThrownAcrossNative) => {
                self.caught = None;
                return Ok(self.shadow_import_rejection(caller));
            }
            Err(error) => return Err(error),
        };
        // The export is read in the realm the module belongs to, and a missing one is not
        // `undefined` here: the specification checks `HasProperty` on the namespace and rejects
        // when it is absent, which is what keeps a typo from resolving with nothing.
        // A namespace is an ordinary property store with no chain to walk, so this is a plain
        // lookup rather than a trapped operation: nothing here can run the program's code.
        if !has_property_checked(
            &self.intrinsics,
            &namespace,
            &Value::String(export.to_owned().into()),
        )? {
            return Ok(self.shadow_import_rejection(caller));
        }
        let value = match self.entering(evaluated, |vm| vm.read_property(&namespace, export, 0)) {
            Ok(value) => value,
            Err(ExecutionError::ThrownAcrossNative) => {
                self.caught = None;
                return Ok(self.shadow_import_rejection(caller));
            }
            Err(error) => return Err(error),
        };
        match self.entering(caller, |vm| vm.wrap_for_realm(&value, caller)) {
            Ok(wrapped) => Ok(Settled::Fulfilled(wrapped)),
            Err(ExecutionError::ThrownAcrossNative) => {
                self.caught = None;
                Ok(self.shadow_import_rejection(caller))
            }
            Err(error) => Err(error),
        }
    }

    /// The one rejection `importValue` has: a `TypeError` belonging to the caller's realm.
    pub(crate) fn shadow_import_rejection(&mut self, caller: RealmId) -> Settled {
        let thrown = self.entering(caller, |vm| {
            Ok(vm.construct_error(
                ErrorKind::Type,
                &[Value::String("ShadowRealm import failed".to_owned().into())],
            ))
        });
        // `entering` cannot fail here: the body is infallible.
        Settled::Rejected(thrown.unwrap_or(Value::Undefined))
    }

    /// Compile and run `source` as a Script in `realm`, keeping the two failures apart.
    ///
    /// `Err(ExecutionError)` is a compile failure or the machine giving up; `Ok(Err(()))` is the
    /// program's own abrupt completion. They are separated because `ShadowRealm` reports them
    /// differently and the difference is observable: source that does not parse throws a
    /// `SyntaxError`, while source that parses and then throws -- including source whose *own*
    /// nested `eval` fails to parse -- throws a `TypeError`. Splitting the compile from the run,
    /// rather than inspecting the error afterwards, is what draws that line in the right place:
    /// a nested `eval`'s `SyntaxError` has already become a thrown value by the time it reaches
    /// the handler installed here, and so cannot be mistaken for this compile's.
    pub(crate) fn shadow_realm_run_units(
        &mut self,
        source: &StringUnits,
        realm: RealmId,
    ) -> Result<Result<Value, ()>, ExecutionError> {
        match source.to_scalar_cow() {
            Ok(text) => self.shadow_realm_run(text.as_ref(), realm),
            Err(_) => {
                let transport = source.to_lossy_string();
                self.shadow_realm_run_with_units(&transport, Some(source), realm)
            }
        }
    }

    pub(crate) fn shadow_realm_run(
        &mut self,
        source: &str,
        realm: RealmId,
    ) -> Result<Result<Value, ()>, ExecutionError> {
        self.shadow_realm_run_with_units(source, None, realm)
    }

    fn shadow_realm_run_with_units(
        &mut self,
        source: &str,
        source_units: Option<&StringUnits>,
        realm: RealmId,
    ) -> Result<Result<Value, ()>, ExecutionError> {
        if self.native_depth >= self.limits.max_native_depth {
            return Err(ExecutionError::CallDepthExceeded {
                limit: self.limits.max_native_depth,
            });
        }
        let caller = std::mem::replace(&mut self.current_realm, realm);
        self.native_depth += 1;
        let depth = self.frames.len();
        // Installed before the compile so that its `frame_depth` is the depth to unwind *to*,
        // which is this one and not the eval frame's. Nothing can throw into it until the
        // compile has succeeded and a frame exists.
        let installed = self.handlers.len();
        self.handlers.push(Handler {
            target: HOST_HANDLER,
            frame_depth: depth,
            stack_height: self.stack.len(),
            native_depth: self.native_depth,
        });
        let outcome = match self.enter_eval_with_units(source, source_units, None, true) {
            Ok(()) => match self.run_frames(depth) {
                Ok(result) => Ok(Ok(result.value)),
                Err(ExecutionError::CaughtByHost | ExecutionError::ThrownAcrossNative) => {
                    // Dropped rather than returned: it is the shadow realm's object.
                    self.caught = None;
                    Ok(Err(()))
                }
                // Anything the language can throw is the program's own outcome; a limit this
                // realm imposes is not, and stays a refusal rather than becoming a TypeError
                // that would read as the program's fault.
                Err(error) if error.as_javascript_error().is_some() => {
                    self.caught = None;
                    Ok(Err(()))
                }
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        };
        self.handlers.truncate(installed);
        self.native_depth -= 1;
        self.current_realm = caller;
        outcome
    }

    /// Call a [`Value::Wrapped`], which is the boundary crossing in both directions at once.
    ///
    /// Arguments are wrapped *into* the target's realm and the result back *out* into the
    /// wrapper's, each by the same [`Vm::wrap_for_realm`] rule, so a non-callable object is
    /// refused whichever way it was travelling. The receiver is dropped: a wrapped function is
    /// always called with `undefined` as `this`, because a `this` would be one more object
    /// crossing the boundary and the specification does not let it.
    pub(crate) fn call_wrapped(
        &mut self,
        wrapped: &Rc<WrappedFunction>,
        arguments: Vec<Value>,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let target_realm = value_realm(&wrapped.target);
        let mut crossed = Vec::with_capacity(arguments.len());
        for argument in &arguments {
            crossed.push(self.wrap_for_realm(argument, target_realm)?);
        }
        let target = wrapped.target.clone();
        let produced =
            self.catching(|vm| vm.call_value(&target, Value::Undefined, crossed, instruction));
        match produced {
            // Wrapped on the way out, in the wrapper's own realm, so a callable result comes
            // back as one more wrapper the caller may hold and a plain object does not come
            // back at all.
            Ok(value) => self.wrap_for_realm(&value, wrapped.realm),
            // The target realm's thrown value, discarded for the reason `evaluate` discards
            // one: it is an object the caller must not reach.
            Err(_) => Err(ExecutionError::TypeErrorMessage {
                message: "a ShadowRealm wrapped function threw".to_owned(),
            }),
        }
    }

    /// The `name` a wrapper copies: the target's when it is a string, and `""` otherwise.
    ///
    /// Unlike `length` this one is a plain `Get`, prototype chain included -- the specification
    /// asks `HasOwnProperty` for `length` and not for `name` -- and a non-string of any kind,
    /// including a symbol, becomes the empty string rather than being coerced.
    pub(crate) fn wrapped_name(&mut self, target: &Value) -> Result<String, ExecutionError> {
        match self.read_property(target, "name", 0)? {
            Value::String(text) => Ok(text.to_lossy_string()),
            _ => Ok(String::new()),
        }
    }

    /// Convert one compiler builtin to the value the current realm exposes.
    ///
    /// Both bytecode `LoadBuiltin` and global-object assembly use this conversion. Keeping the
    /// enum-to-native mapping here means adding a builtin cannot update one path while leaving the
    /// other with a stale constructor or realm tag.
    pub(crate) fn builtin_value(&mut self, builtin: kiln_ir::Builtin, realm: RealmId) -> Value {
        match builtin {
            kiln_ir::Builtin::GlobalThis => self.global_object(),
            kiln_ir::Builtin::Process => self.process_object(),
            kiln_ir::Builtin::NotANumber => Value::Number(f64::NAN),
            kiln_ir::Builtin::Infinity => Value::Number(f64::INFINITY),
            kiln_ir::Builtin::Error => Value::Native(Native::Error(ErrorKind::Error), realm),
            kiln_ir::Builtin::TypeError => Value::Native(Native::Error(ErrorKind::Type), realm),
            kiln_ir::Builtin::RangeError => Value::Native(Native::Error(ErrorKind::Range), realm),
            kiln_ir::Builtin::ReferenceError => {
                Value::Native(Native::Error(ErrorKind::Reference), realm)
            }
            kiln_ir::Builtin::SyntaxError => Value::Native(Native::Error(ErrorKind::Syntax), realm),
            kiln_ir::Builtin::UriError => Value::Native(Native::Error(ErrorKind::Uri), realm),
            kiln_ir::Builtin::EvalError => Value::Native(Native::Error(ErrorKind::Eval), realm),
            kiln_ir::Builtin::SuppressedError => {
                Value::Native(Native::Error(ErrorKind::Suppressed), realm)
            }
            kiln_ir::Builtin::AggregateError => {
                Value::Native(Native::Error(ErrorKind::Aggregate), realm)
            }
            kiln_ir::Builtin::Symbol => Value::Native(Native::SymbolConstructor, realm),
            kiln_ir::Builtin::String => Value::Native(Native::StringConstructor, realm),
            kiln_ir::Builtin::Set => Value::Native(Native::SetConstructor, realm),
            kiln_ir::Builtin::Map => Value::Native(Native::MapConstructor, realm),
            kiln_ir::Builtin::Buffer => Value::Native(Native::NodeBufferGlobal, realm),
            // The shared TypeScript `URL` (`kiln-builtins/src/url.ts`) assigns itself onto the
            // global object in Node mode, and the bare name has to find that class rather than
            // the Rust constructor it replaces -- as it would in node, where `URL` is a global
            // like any other and a polyfill's assignment is honoured.
            kiln_ir::Builtin::Url => {
                // Only a global object that already exists is asked: building one publishes
                // `URL` through this very arm.
                let existing = match &self.module_realm {
                    Some(shared) => shared
                        .borrow()
                        .global_objects
                        .get(realm.0 as usize)
                        .and_then(|slot| slot.clone()),
                    None => self
                        .global_objects
                        .get(realm.0 as usize)
                        .and_then(|slot| slot.clone()),
                };
                if let Some(Value::Object(object)) = existing
                    && let Some(assigned @ Value::Function(_)) = object.borrow().get_own("URL")
                {
                    return assigned.clone();
                }
                Value::Native(Native::UrlConstructor, realm)
            }
            kiln_ir::Builtin::TextDecoder => Value::Native(Native::TextDecoderConstructor, realm),
            kiln_ir::Builtin::Array => Value::Native(Native::ArrayConstructor, realm),
            kiln_ir::Builtin::Object => Value::Native(Native::ObjectConstructor, realm),
            kiln_ir::Builtin::Json => Value::Native(Native::Json, realm),
            kiln_ir::Builtin::Math => Value::Native(Native::Math, realm),
            kiln_ir::Builtin::Atomics => Value::Native(Native::Atomics, realm),
            kiln_ir::Builtin::Number => Value::Native(Native::NumberConstructor, realm),
            kiln_ir::Builtin::Boolean => Value::Native(Native::BooleanConstructor, realm),
            kiln_ir::Builtin::ParseInt => Value::Native(Native::ParseInt, realm),
            kiln_ir::Builtin::Escape => Value::Native(Native::Escape, realm),
            kiln_ir::Builtin::Unescape => Value::Native(Native::Unescape, realm),
            kiln_ir::Builtin::EncodeUri => Value::Native(Native::EncodeUri, realm),
            kiln_ir::Builtin::EncodeUriComponent => {
                Value::Native(Native::EncodeUriComponent, realm)
            }
            kiln_ir::Builtin::DecodeUri => Value::Native(Native::DecodeUri, realm),
            kiln_ir::Builtin::DecodeUriComponent => {
                Value::Native(Native::DecodeUriComponent, realm)
            }
            kiln_ir::Builtin::ParseFloat => Value::Native(Native::ParseFloat, realm),
            kiln_ir::Builtin::IsNaN => Value::Native(Native::IsNaN, realm),
            kiln_ir::Builtin::IsFinite => Value::Native(Native::IsFinite, realm),
            kiln_ir::Builtin::SetTimeout => Value::Native(Native::SetTimeout, realm),
            kiln_ir::Builtin::SetInterval => Value::Native(Native::SetInterval, realm),
            kiln_ir::Builtin::ClearTimeout => Value::Native(Native::ClearTimeout, realm),
            kiln_ir::Builtin::ClearInterval => Value::Native(Native::ClearInterval, realm),
            kiln_ir::Builtin::QueueMicrotask => Value::Native(Native::QueueMicrotask, realm),
            kiln_ir::Builtin::SetImmediate => Value::Native(Native::SetImmediate, realm),
            kiln_ir::Builtin::ClearImmediate => Value::Native(Native::ClearImmediate, realm),
            kiln_ir::Builtin::Promise => Value::Native(Native::PromiseConstructor, realm),
            kiln_ir::Builtin::RegExp => Value::Native(Native::RegExpConstructor, realm),
            kiln_ir::Builtin::Date => Value::Native(Native::DateConstructor, realm),
            kiln_ir::Builtin::WeakMap => Value::Native(Native::WeakMapConstructor, realm),
            kiln_ir::Builtin::WeakSet => Value::Native(Native::WeakSetConstructor, realm),
            kiln_ir::Builtin::Performance => Value::Native(Native::Performance, realm),
            kiln_ir::Builtin::Reflect => Value::Native(Native::Reflect, realm),
            kiln_ir::Builtin::BigInt => Value::Native(Native::BigIntConstructor, realm),
            kiln_ir::Builtin::Function => Value::Native(Native::FunctionConstructor, realm),
            kiln_ir::Builtin::Eval => Value::Native(Native::Eval, realm),
            kiln_ir::Builtin::Proxy => Value::Native(Native::ProxyConstructor, realm),
            kiln_ir::Builtin::DisposableStack => {
                Value::Native(Native::DisposableStackConstructor, realm)
            }
            kiln_ir::Builtin::AsyncDisposableStack => {
                Value::Native(Native::AsyncDisposableStackConstructor, realm)
            }
            kiln_ir::Builtin::AbortController => {
                Value::Native(Native::AbortControllerConstructor, realm)
            }
            kiln_ir::Builtin::AbortSignal => Value::Native(Native::AbortSignalConstructor, realm),
            kiln_ir::Builtin::TextEncoder => Value::Native(Native::TextEncoderConstructor, realm),
            // Defined on the global object by the shared bootstrap
            // (`kiln-builtins/src/structured_clone.ts`), and read from there, so a program that
            // replaces `globalThis.structuredClone` is seen by the bare name too.
            kiln_ir::Builtin::StructuredClone => match self.global_object_in(realm) {
                Value::Object(globals) => globals
                    .borrow()
                    .get_own("structuredClone")
                    .cloned()
                    .unwrap_or(Value::Undefined),
                _ => Value::Undefined,
            },
            kiln_ir::Builtin::ArrayBuffer => Value::Native(Native::ArrayBufferConstructor, realm),
            kiln_ir::Builtin::Int8Array => {
                Value::Native(Native::TypedArrayConstructor(TypedKind::Int8), realm)
            }
            kiln_ir::Builtin::Uint8Array => {
                Value::Native(Native::TypedArrayConstructor(TypedKind::Uint8), realm)
            }
            kiln_ir::Builtin::Uint8ClampedArray => Value::Native(
                Native::TypedArrayConstructor(TypedKind::Uint8Clamped),
                realm,
            ),
            kiln_ir::Builtin::Int16Array => {
                Value::Native(Native::TypedArrayConstructor(TypedKind::Int16), realm)
            }
            kiln_ir::Builtin::Uint16Array => {
                Value::Native(Native::TypedArrayConstructor(TypedKind::Uint16), realm)
            }
            kiln_ir::Builtin::Int32Array => {
                Value::Native(Native::TypedArrayConstructor(TypedKind::Int32), realm)
            }
            kiln_ir::Builtin::Uint32Array => {
                Value::Native(Native::TypedArrayConstructor(TypedKind::Uint32), realm)
            }
            kiln_ir::Builtin::Float32Array => {
                Value::Native(Native::TypedArrayConstructor(TypedKind::Float32), realm)
            }
            kiln_ir::Builtin::Float64Array => {
                Value::Native(Native::TypedArrayConstructor(TypedKind::Float64), realm)
            }
        }
    }

    /// The realm's global object, built and bootstrapped: the one way a realm comes into use.
    ///
    /// [`Vm::global_object_in`] builds the machine's own furniture and nothing else, because it
    /// is reached lazily from a hundred places that cannot run program code. This is the step
    /// after it: the embedder's [`RealmBootstrap`] -- the TypeScript built-ins ADR 0060 gives
    /// one owner -- run to completion in `realm`, once. Every place a realm begins (an
    /// execution's entry realm, `$262.createRealm`, `new ShadowRealm`) calls this before any
    /// program code can observe the realm, so a realm with half its built-ins is never seen.
    ///
    /// "Once" is decided by whether the global object already existed, which the realm tables
    /// shared by a module session answer for every machine in the session: the first module to
    /// run bootstraps the realm and the rest find it built.
    pub(crate) fn realize_realm(&mut self, realm: RealmId) -> Result<Value, ExecutionError> {
        if let Some(existing) = self.built_global_object(realm) {
            return Ok(existing);
        }
        let global = self.global_object_in(realm);
        let Some(bootstrap) = self.realm_bootstrap else {
            return Ok(global);
        };
        let program = bootstrap().map_err(|message| ExecutionError::Host {
            instruction: 0,
            message: format!("kiln: the realm bootstrap did not compile: {message}"),
        })?;
        let locals = vec![Value::Undefined; program.local_count as usize];
        // A module that imports itself refreshes its namespace from the *running* locals between
        // instructions; the bootstrap's locals are not that module's, so the refresh stands aside
        // until the bootstrap has returned.
        let self_bindings = std::mem::take(&mut self.self_bindings);
        let self_namespaces = std::mem::take(&mut self.self_namespaces);
        let outcome = self.entering(realm, |vm| {
            vm.drive_module(
                program,
                locals,
                Some(Rc::from(REALM_BOOTSTRAP_PATH)),
                0,
                None,
            )
        });
        self.self_bindings = self_bindings;
        self.self_namespaces = self_namespaces;
        let thrown = match outcome {
            Ok(Ok(_)) => {
                // An engine module whose run built this realm opened its window before the
                // global object existed; the bootstrap has since removed the name, so open it.
                if realm.0 == 0
                    && let Some(shared) = &self.module_realm
                    && let Value::Object(object) = &global
                {
                    let mut shared = shared.borrow_mut();
                    if let Some(window @ PrivateWindow::Absent) = shared.private_windows.last_mut()
                    {
                        *window = open_private_window(&mut object.borrow_mut());
                    }
                }
                return Ok(global);
            }
            Ok(Err(thrown)) => thrown,
            Err(ExecutionError::ThrownAcrossNative) => {
                self.caught.take().unwrap_or(Value::Undefined)
            }
            Err(error) => return Err(error),
        };
        let instruction = self.instruction;
        Err(ExecutionError::Uncaught {
            instruction,
            message: format!(
                "kiln: the realm bootstrap threw: {}",
                self.describe_uncaught(&thrown, instruction)
            ),
            error_constructor: self.error_constructor(&thrown),
            trace: captured_trace(&thrown),
        })
    }

    /// The realm's global object if it has been built, by this machine or another in its session.
    fn built_global_object(&mut self, realm: RealmId) -> Option<Value> {
        if let Some(shared) = &self.module_realm {
            let existing = shared
                .borrow()
                .global_objects
                .get(realm.0 as usize)
                .cloned()
                .flatten()?;
            if self.global_objects.len() <= realm.0 as usize {
                self.global_objects.resize(realm.0 as usize + 1, None);
            }
            self.global_objects[realm.0 as usize] = Some(existing.clone());
            return Some(existing);
        }
        self.global_objects
            .get(realm.0 as usize)
            .and_then(|o| o.as_ref())
            .cloned()
    }

    /// `realm`'s global object, built on first use and shared from then on.
    ///
    /// Every binding it publishes is minted *in that realm*, which is the whole of what makes
    /// a second realm a second realm: `other.Array` and `Array` name the same built-in and are
    /// not the same function, so `[] instanceof other.Array` is false while `Array.isArray([])`
    /// stays true.
    pub(crate) fn global_object_in(&mut self, realm: RealmId) -> Value {
        if let Some(existing) = self.built_global_object(realm) {
            return existing;
        }
        // Every one of these is *non-enumerable*, which is what node does and what stops
        // `Object.keys(globalThis)` from being a list of the realm's furniture. A program's own
        // `globalThis.x = 1` is enumerable, which is the distinction that makes the question
        // worth asking at all.
        let mut globals = ObjectData::default();
        for kind in ErrorKind::ALL {
            globals.set_non_enumerable(kind.name(), Value::Native(Native::Error(kind), realm));
        }
        for &(builtin, _) in kiln_ir::Builtin::GLOBAL_BINDINGS {
            if matches!(
                builtin,
                kiln_ir::Builtin::Error
                    | kiln_ir::Builtin::TypeError
                    | kiln_ir::Builtin::RangeError
                    | kiln_ir::Builtin::ReferenceError
                    | kiln_ir::Builtin::SyntaxError
                    | kiln_ir::Builtin::UriError
                    | kiln_ir::Builtin::EvalError
                    | kiln_ir::Builtin::SuppressedError
                    | kiln_ir::Builtin::AggregateError
                    | kiln_ir::Builtin::GlobalThis
                    | kiln_ir::Builtin::Process
                    | kiln_ir::Builtin::StructuredClone
                    | kiln_ir::Builtin::NotANumber
                    | kiln_ir::Builtin::Infinity
            ) {
                continue;
            }
            globals.set_non_enumerable(builtin.name(), self.builtin_value(builtin, realm));
        }
        globals.set_non_enumerable(
            "Temporal",
            Value::Native(Native::Temporal(temporal::Op::Namespace), realm),
        );
        globals.set_non_enumerable("WeakRef", Value::Native(Native::WeakRefConstructor, realm));
        globals.set_non_enumerable(
            "ShadowRealm",
            Value::Native(Native::ShadowRealmConstructor, realm),
        );
        globals.set_non_enumerable(
            "FinalizationRegistry",
            Value::Native(Native::FinalizationRegistryConstructor, realm),
        );
        // The realm's engine-private object (`__kilnPrivate`), kept in an internal slot. The
        // bootstrap captures it from the global name and deletes the name before any program
        // line runs; a realm that runs no bootstrap never publishes the name at all.
        let private = new_engine_private(realm);
        if let Value::Object(object) = &private {
            object
                .borrow_mut()
                .set_non_enumerable("process", self.process_object());
        }
        globals.define_value(ENGINE_PRIVATE_SLOT, private.clone(), Attributes::BUILT_IN);
        if self.realm_bootstrap.is_some() {
            globals.set_non_enumerable("__kilnPrivate", private);
        }
        globals.set_non_enumerable(
            "Intl",
            Value::Native(Native::Intl(intl::Op::Namespace), realm),
        );
        globals.set_non_enumerable("crypto", Value::Native(Native::NodeCrypto, realm));
        globals.set_non_enumerable(
            "__kilnWebCryptoPrimitive",
            Value::Native(Native::WebCryptoPrimitive, realm),
        );
        globals.set_non_enumerable(
            "__kilnCompressionPrimitive",
            Value::Native(Native::WebCompressionPrimitive, realm),
        );
        globals.set_non_enumerable(
            "SharedArrayBuffer",
            Value::Native(Native::SharedArrayBufferConstructor, realm),
        );
        globals.set_non_enumerable(
            "DataView",
            Value::Native(Native::DataViewConstructor, realm),
        );
        globals.set_non_enumerable(
            "Iterator",
            Value::Native(Native::IteratorConstructor, realm),
        );
        globals.set_non_enumerable(
            "Float16Array",
            Value::Native(Native::TypedArrayConstructor(TypedKind::Float16), realm),
        );
        globals.set_non_enumerable(
            "BigInt64Array",
            Value::Native(Native::TypedArrayConstructor(TypedKind::BigInt64), realm),
        );
        globals.set_non_enumerable(
            "BigUint64Array",
            Value::Native(Native::TypedArrayConstructor(TypedKind::BigUint64), realm),
        );
        // On the global object in every mode, as node has it: the bare name resolved everywhere
        // while `globalThis.process` read `undefined` until 2026-09-19. The eleven web classes
        // that stood here as hollow stubs (`new Event("x").type` was `undefined`) are the shared
        // TypeScript in `kiln-builtins/src/web.ts` now, which both engines run under the adapter.
        globals.set_non_enumerable("process", self.process_object());
        if self.async_host.is_some() {
            // Captured and deleted by the shared Buffer bootstrap (`kiln-builtins/src/buffer.ts`),
            // which runs in Node mode only -- so it is published in Node mode only, and neither
            // engine's global surface carries it afterwards.
            globals.set_non_enumerable(
                "__kilnBufferToString",
                Value::Native(Native::NodeBufferToString, realm),
            );
        }
        // Whether this realm installs kiln's own members (ADR 0068), for the shared bootstrap's
        // `%TypedArray%.prototype`, which installs its extension itself. Captured and deleted by
        // `kiln-builtins/src/extensions.ts` before any other bootstrap line runs.
        globals.set_non_enumerable(
            "__kilnExtensions",
            Value::Boolean(kiln_builtin_surface::kiln_extensions_installed()),
        );
        // The four bridges the shared `URL` (`kiln-builtins/src/url.ts`) captures and deletes;
        // the URL bootstrap runs in every mode, as the native runtime's does.
        for (name, native) in [
            ("__kilnUrlParse", Native::UrlParseRaw),
            ("__kilnUrlSet", Native::UrlSetRaw),
            ("__kilnUrlFast", Native::UrlFastRaw),
            ("__kilnUrlAdopt", Native::UrlAdoptRaw),
            // The transfer primitive `arraybuffer.ts` captures and deletes (every realm).
            ("__kilnArrayBufferDetach", Native::ArrayBufferDetachRaw),
            (
                "__kilnArrayBufferByteLength",
                Native::ArrayBufferByteLengthRaw,
            ),
            // The in-place length move `resize` and `grow` make (`arraybuffer.ts`, every realm).
            ("__kilnArrayBufferResize", Native::ArrayBufferResizeRaw),
            // Whether a view has left its buffer (`typedarray.ts`, every realm).
            (
                "__kilnTypedArrayOutOfBounds",
                Native::TypedArrayOutOfBoundsRaw,
            ),
            ("__kilnDataViewState", Native::DataViewStateRaw),
            ("__kilnIsConstructor", Native::IsConstructorRaw),
            ("__kilnCollectionSize", Native::CollectionSizeRaw),
            // A same-type element copy and the default-order sort (`typedarray.ts`, every realm).
            ("__kilnTypedArrayCopy", Native::TypedArrayCopyRaw),
            ("__kilnTypedArraySort", Native::TypedArraySortRaw),
            // `[[SetPrototypeOf]]` and `[[PreventExtensions]]` answering their boolean, which the
            // shared `Reflect` methods (`object_reflect.ts`) cannot read off `Object`'s spellings:
            // those turn `false` into a TypeError indistinguishable from a proxy trap's own.
            ("__kilnSetPrototypeOf", Native::ReflectSetPrototypeOf),
            ("__kilnPreventExtensions", Native::ReflectPreventExtensions),
            // The structured clone's plain-data fast path: measured for the native backend
            // only, so here it always answers "not handled" and the shared TypeScript clone runs.
            ("__kilnCloneData", Native::CloneDataRaw),
            // Whether a microtask checkpoint would run anything now, which message delivery
            // (`kiln-builtins/src/web_events_streams.ts`) asks before its next message.
            ("__kilnJobsPending", Native::JobsPendingRaw),
            // The poll phase's lane, where message delivery (the same file) is queued.
            ("__kilnQueueIo", Native::QueueIoRaw),
            // Whether a timer keeps the loop alive: `ref`/`unref` on the timer objects of
            // `kiln-builtins/src/timers.ts`, which captures it.
            ("__kilnTimerRef", Native::TimerRefRaw),
            // The runtime's cross-realm ports and workers, which the same file captures.
            ("__kilnWorkerOp", Native::WorkerOpRaw),
            ("__kilnPortWait", Native::PortWaitRaw),
            // Node's unhandled-rejection tracking registers itself here
            // (`kiln-builtins/src/rejections.ts`).
            ("__kilnPromiseRejections", Native::PromiseRejectionsRaw),
            ("__kilnRegExpLegacyCapture", Native::RegExpLegacyCaptureRaw),
        ] {
            globals.set_non_enumerable(name, Value::Native(native, realm));
        }
        // `structuredClone` is the shared bootstrap's (`kiln-builtins/src/structured_clone.ts`),
        // defined in every realm, as the native runtime's is.

        // The global functions, which a program can also reach by their bare names -- and has to
        // find the *same* value both ways: `globalThis.parseInt === parseInt`.
        let mut console = ObjectData::default();
        console.set_prototype(Value::Native(
            Native::PrototypeOf(PrototypeTable::Console),
            realm,
        ));
        console.set_non_enumerable("Console", Value::Native(Native::ConsoleConstructor, realm));
        console.set_non_enumerable("log", Value::Native(Native::ConsoleLog, realm));
        console.set_non_enumerable("info", Value::Native(Native::ConsoleInfo, realm));
        console.set_non_enumerable("debug", Value::Native(Native::ConsoleDebug, realm));
        console.set_non_enumerable("dir", Value::Native(Native::ConsoleDir, realm));
        // Host ABI v3 exposes a captured standard-*error* operation beside the standard-output
        // one, so error delivery is no longer folded onto stdout. Both still cross the same
        // explicit boundary, which is what preserves the realm's no-ambient-I/O guarantee while
        // making console an ordinary cross-module value.
        console.set_non_enumerable("error", Value::Native(Native::ConsoleError, realm));
        console.set_non_enumerable("warn", Value::Native(Native::ConsoleWarn, realm));
        console.set_non_enumerable("assert", Value::Native(Native::ConsoleAssert, realm));
        console.set_non_enumerable("clear", Value::Native(Native::ConsoleClear, realm));
        console.set_non_enumerable("count", Value::Native(Native::ConsoleCount, realm));
        console.set_non_enumerable(
            "countReset",
            Value::Native(Native::ConsoleCountReset, realm),
        );
        console.set_non_enumerable("group", Value::Native(Native::ConsoleGroup, realm));
        console.set_non_enumerable("groupEnd", Value::Native(Native::ConsoleGroupEnd, realm));
        console.set_non_enumerable(
            "groupCollapsed",
            Value::Native(Native::ConsoleGroupCollapsed, realm),
        );
        console.set_non_enumerable("table", Value::Native(Native::ConsoleTable, realm));
        console.set_non_enumerable("time", Value::Native(Native::ConsoleTime, realm));
        console.set_non_enumerable("timeEnd", Value::Native(Native::ConsoleTimeEnd, realm));
        console.set_non_enumerable("timeLog", Value::Native(Native::ConsoleTimeLog, realm));
        console.set_non_enumerable("trace", Value::Native(Native::ConsoleTrace, realm));
        console.set_non_enumerable("dirxml", Value::Native(Native::ConsoleDirxml, realm));
        console.set_non_enumerable("timeStamp", Value::Native(Native::ConsoleTimeStamp, realm));
        console.set_non_enumerable("profile", Value::Native(Native::ConsoleProfile, realm));
        console.set_non_enumerable(
            "profileEnd",
            Value::Native(Native::ConsoleProfileEnd, realm),
        );
        globals.set_non_enumerable("console", Value::Object(Rc::new(RefCell::new(console))));
        // The three value properties of the global object are the only ones the specification
        // locks on every axis: `{ writable: false, enumerable: false, configurable: false }`.
        // That is not decoration -- it is what makes `delete NaN` answer `false` and
        // `globalThis.undefined = 1` a silent no-op in sloppy code and a TypeError in strict.
        globals.define_value("NaN", Value::Number(f64::NAN), Attributes::NONE);
        globals.define_value("Infinity", Value::Number(f64::INFINITY), Attributes::NONE);
        globals.define_value("undefined", Value::Undefined, Attributes::NONE);
        let object = Rc::new(RefCell::new(globals));
        let value = Value::Object(Rc::clone(&object));
        // The global object holds itself under both of its names, which is how a program reaches
        // it without a keyword: `global.Object === Object` is the standard test for "am I in a
        // realm that has one".
        object
            .borrow_mut()
            .set_non_enumerable("globalThis", value.clone());
        object
            .borrow_mut()
            .set_non_enumerable("global", value.clone());
        // The host's, not the language's: present only because the runner asked for it, and
        // spelled with the `$` that no ECMAScript name uses so it cannot shadow one. The `$262`
        // object that wraps it is assembled by the runner, which is where a statement about what
        // this host can do belongs.
        if self.host_realms {
            object.borrow_mut().set_non_enumerable(
                "$262CreateRealm",
                Value::Native(Native::HostCreateRealm, realm),
            );
            // Published per realm and carrying it, so `other.$262EvalScript(src)` enters that
            // realm before it runs and its declarations land on *that* global object.
            object.borrow_mut().set_non_enumerable(
                "$262EvalScript",
                Value::Native(Native::HostEvalScript, realm),
            );
        }
        // The agent protocol, on the same terms: published only because the host asked, and only
        // as the eight operations. A realm made by `$262.createRealm` gets them too -- an agent
        // is a property of the *agent*, not of one of its realms, and a program that made a
        // second realm and then started an agent from it is asking the same host the same thing.
        if self.host_agents.is_some() {
            for op in AgentOp::ALL {
                object.borrow_mut().set_non_enumerable(
                    op.hook_name(),
                    Value::Native(Native::HostAgent(op), realm),
                );
            }
        }
        if let Some(shared) = &self.module_realm {
            let mut shared = shared.borrow_mut();
            if shared.global_objects.len() <= realm.0 as usize {
                shared.global_objects.resize(realm.0 as usize + 1, None);
            }
            shared.global_objects[realm.0 as usize] = Some(value.clone());
        }
        if self.global_objects.len() <= realm.0 as usize {
            self.global_objects.resize(realm.0 as usize + 1, None);
        }
        self.global_objects[realm.0 as usize] = Some(value.clone());
        value
    }
}
