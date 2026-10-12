use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    fn stack_function_name(closure: &Closure) -> Option<String> {
        closure
            .properties
            .borrow()
            .get_own("name")
            .and_then(|value| match value {
                Value::String(name) if !name.is_empty() => Some(name.to_lossy_string()),
                _ => None,
            })
            .or_else(|| {
                closure
                    .name
                    .as_deref()
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            })
            .or_else(|| {
                closure
                    .inferred_name
                    .as_ref()
                    .filter(|name| !name.is_empty())
                    .map(|name| name.to_lossy_string())
            })
    }

    /// How a stack frame names its module: relative to the project root when it is under it,
    /// which is the form the native backend bakes in. Anything else keeps the name it has.
    fn frame_file_name(&self, path: Option<&str>) -> String {
        let Some(path) = path else {
            return "<kiln>".to_owned();
        };
        if let Some(root) = self.display_root.as_deref() {
            let root = root.trim_end_matches('/');
            if let Some(relative) = path
                .strip_prefix(root)
                .and_then(|rest| rest.strip_prefix('/'))
                && !relative.is_empty()
            {
                return relative.to_owned();
            }
        }
        path.to_owned()
    }

    /// A module body that has already returned. Jobs run after it (a promise continuation, a
    /// timer) are called on top of its leftover state, which is not a caller: nothing is waiting
    /// in it. A real module frame is mid-call, so its last instruction is never the final
    /// `Return`.
    fn idle_module_frame(
        function: Option<&Rc<Closure>>,
        program: &Program,
        instruction: usize,
    ) -> bool {
        function.is_none()
            && instruction + 1 == program.code.len()
            && matches!(program.code.get(instruction), Some(Instruction::Return))
    }

    fn error_stack_frame_limit(&self) -> Option<usize> {
        let constructor = Value::Native(Native::Error(ErrorKind::Error), self.current_realm);
        let identity = intrinsic_key(&constructor)?;
        let entries = self.intrinsics.entries.borrow();
        let index = self.intrinsics.entry_position(&entries, identity)?;
        let value = entries.get(index)?.1.get_own("stackTraceLimit")?;
        let Value::Number(limit) = value else {
            return None;
        };
        if limit.is_nan() || *limit <= 0.0 {
            return Some(0);
        }
        Some(if limit.is_infinite() || *limit >= usize::MAX as f64 {
            usize::MAX
        } else {
            limit.floor() as usize
        })
    }

    /// Resume a generator, recovering what it threw rather than unwinding past this point.
    pub(super) fn resume_catching(
        &mut self,
        generator: &Rc<GeneratorData>,
        resume: Resume,
        instruction: usize,
    ) -> Result<Result<(Value, bool), Value>, ExecutionError> {
        // A `.return()` unwinds by throwing a marker, and the host handler installed just below
        // catches everything -- including that marker, which is not an exception. What it means
        // is that the body ran out of `finally` blocks and finished with the value the consumer
        // asked to return, so the value has to be kept here to answer with.
        let returning = match &resume {
            Resume::Return(value) => Some(value.clone()),
            // A `Send` that resumes an unwind already in flight is still that unwind: what is
            // finishing is the `finally` block which awaited, not the body.
            Resume::Send(_) | Resume::Throw(_) => generator.borrow().returning.clone(),
        };
        let installed = self.handlers.len();
        self.handlers.push(Handler {
            target: HOST_HANDLER,
            frame_depth: self.frames.len(),
            stack_height: self.stack.len(),
            native_depth: self.native_depth,
        });
        let outcome = self.resume_generator_with(generator, resume, instruction);
        self.handlers.truncate(installed);
        match outcome {
            // Suspended again with the unwind still travelling: remember it, so the resumption
            // that finishes the `finally` recognises the marker as this return rather than as a
            // thrown value.
            Ok(value @ (_, false)) => {
                generator.borrow_mut().returning = returning;
                Ok(Ok(value))
            }
            Ok(value) => {
                generator.borrow_mut().returning = None;
                Ok(Ok(value))
            }
            Err(ExecutionError::CaughtByHost) => {
                let caught = self.caught.take().unwrap_or(Value::Undefined);
                if let (Value::Native(Native::ReturnSignal, _), Some(value)) = (&caught, returning)
                {
                    let mut state = generator.borrow_mut();
                    state.complete();
                    drop(state);
                    return Ok(Ok((value, true)));
                }
                generator.borrow_mut().returning = None;
                Ok(Err(caught))
            }
            Err(ExecutionError::GeneratorReturned) => {
                let mut state = generator.borrow_mut();
                state.complete();
                drop(state);
                Ok(Ok((returning.unwrap_or(Value::Undefined), true)))
            }
            Err(error) => Err(error),
        }
    }

    /// Settle a promise, queueing whatever was waiting on it.
    ///
    /// Only the first settlement counts: a promise is a value that arrives once, and both
    /// `resolve` and `reject` are handed to the program, which may call either twice.
    /// Take the result of a step, turning an error a program could catch into a thrown value.
    ///
    /// `Ok(None)` means control has already moved to a handler and the caller should go round the
    /// loop again rather than using a result that does not exist.
    pub(super) fn catchable<T>(
        &mut self,
        result: Result<T, ExecutionError>,
        instruction: usize,
    ) -> Result<Option<T>, ExecutionError> {
        match result {
            Ok(value) => Ok(Some(value)),
            // The Rust stack is flat again here, so the handler that could not be reached
            // from inside a callback can take the value now.
            Err(ExecutionError::ThrownAcrossNative) => {
                let thrown = self.caught.take().unwrap_or(Value::Undefined);
                self.unwind(thrown, instruction)?;
                Ok(None)
            }
            Err(error) => match error.as_javascript_error() {
                Some((kind, message)) => {
                    self.throw_error(kind, message, instruction)?;
                    Ok(None)
                }
                None => Err(error),
            },
        }
    }

    // ----------------------------------------------------------------------------------------
    // The generic `Array.prototype` walk.
    //
    // Every method on `Array.prototype` is defined on an arbitrary object rather than on an
    // array: each step is `HasProperty(O, k)`, `Get(O, k)`, `Set(O, k, v, true)` or
    // `DeletePropertyOrThrow(O, k)` against the receiver the program supplied, and the bound
    // comes from `Get(O, "length")`. `Array.prototype.slice.call(arguments)` is why they are
    // written that way, and the whole family below is what a redirected call reaches.
    //
    // A copy walked in the receiver's place answers from a snapshot, and three things are then
    // wrong at once: a mutating method writes nothing back, an index the walk deleted or that
    // only the prototype carries is visited anyway, and the receiver a callback is handed is an
    // array the program never saw. An array receiver keeps its dense fast path, which is the
    // same walk with the indirection removed.
    // ----------------------------------------------------------------------------------------
}

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// The call stack as names, innermost first, at the moment this error object was built.
    ///
    /// A closure that was never given a name shows as `<anonymous>` -- which is most of them in
    /// a bundle, but the named ones between them are usually enough to locate the call.
    pub(super) fn trace(&self) -> Vec<String> {
        std::iter::once(self.function.as_ref())
            .chain(
                self.frames
                    .iter()
                    .rev()
                    .map(|frame| frame.function.as_ref()),
            )
            .map(|function| match function {
                Some(closure) => closure.name.as_deref().unwrap_or("<anonymous>").to_owned(),
                None => "<module>".to_owned(),
            })
            .collect()
    }

    /// Private frame snapshot primitive used by the public TypeScript CallSite facade.
    pub(super) fn capture_stack_trace(
        &mut self,
        arguments: &[Value],
        _instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let target = arguments.first().cloned().unwrap_or(Value::Undefined);
        let registered_accessors = self.intrinsics.error_stack_accessors(self.current_realm);
        if matches!(&target, Value::Native(Native::Error(ErrorKind::Error), _))
            && arguments.len() >= 5
            && matches!(arguments.get(3), Some(Value::Object(_)))
            && matches!(arguments.get(4), Some(Value::Object(_)))
        {
            if let (Some(getter), Some(setter), Some(brand_token)) =
                (arguments.get(1), arguments.get(2), arguments.get(4))
            {
                self.intrinsics.register_error_stack_accessors(
                    self.current_realm,
                    ErrorStackAccessors {
                        getter: getter.clone(),
                        setter: setter.clone(),
                        brand_token: brand_token.clone(),
                    },
                );
            }
            return Ok(Value::Undefined);
        }
        if let Some(token) = registered_accessors
            .as_ref()
            .map(|value| &value.brand_token)
            && arguments
                .get(2)
                .is_some_and(|candidate| strict_equal(candidate, token))
        {
            return Ok(Value::Boolean(matches!(
                &target,
                Value::Object(object) if object.borrow().error_brand
            )));
        }
        if matches!(arguments.get(2), Some(Value::Boolean(true))) {
            return Ok(match &target {
                Value::Object(object) if object.borrow().error_brand => object
                    .borrow()
                    .error_stack_frames
                    .clone()
                    .unwrap_or(Value::Undefined),
                _ => Value::Undefined,
            });
        }
        if !is_object_type(&target) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "invalid argument to Error.captureStackTrace".to_owned(),
            });
        }
        if let Some(constructor) = arguments.get(1)
            && !matches!(constructor, Value::Undefined)
            && !is_callable(constructor)
        {
            return Err(ExecutionError::TypeErrorMessage {
                message: "invalid constructorOpt to Error.captureStackTrace".to_owned(),
            });
        }
        let mut frames = Vec::with_capacity(self.frames.len());
        // This primitive is called by the public TypeScript `Error.captureStackTrace`
        // facade. Like the native frame-snapshot primitive, omit that implementation
        // frame so the first CallSite is the caller of the facade.
        frames.extend(
            self.frames
                .iter()
                .rev()
                .filter(|frame| {
                    !Self::idle_module_frame(
                        frame.function.as_ref(),
                        &frame.program,
                        frame.instruction.saturating_sub(1),
                    )
                })
                .map(|frame| {
                    let span = frame
                        .function
                        .as_ref()
                        .and_then(|closure| frame.program.functions.get(closure.function as usize))
                        .map(|proto| proto.span);
                    let site = frame
                        .program
                        .debug_call_site(span, frame.instruction.saturating_sub(1));
                    (
                        frame.function.clone(),
                        frame.method_name.clone(),
                        self.frame_file_name(frame.module_path.as_deref()),
                        site.map(|site| (site.line, site.column)),
                    )
                }),
        );
        // The primitive receives no public JavaScript CallSite objects. The facade is built in
        // errors.ts from these copied metadata records and cannot keep VM frames alive.
        if let Some(Value::Function(constructor)) = arguments.get(1)
            && let Some(index) = frames.iter().position(|frame| {
                frame
                    .0
                    .as_ref()
                    .is_some_and(|candidate| Rc::ptr_eq(candidate, constructor))
            })
        {
            frames.drain(..=index);
        }
        Ok(Value::Array(ArrayData::new(
            frames
                .into_iter()
                .map(|(frame, method_name, file_name, location)| {
                    ordinary_object([
                        ("fileName".to_owned(), Value::String(file_name.into())),
                        (
                            "functionName".to_owned(),
                            frame
                                .and_then(|closure| Self::stack_function_name(closure.as_ref()))
                                .or_else(|| method_name.as_ref().map(|name| name.to_lossy_string()))
                                .map_or(Value::Undefined, |name| Value::String(name.into())),
                        ),
                        (
                            "methodName".to_owned(),
                            method_name.map_or(Value::Undefined, Value::String),
                        ),
                        ("instruction".to_owned(), Value::Number(0.0)),
                        (
                            "lineNumber".to_owned(),
                            location
                                .map_or(Value::Undefined, |(line, _)| Value::Number(line as f64)),
                        ),
                        (
                            "columnNumber".to_owned(),
                            location.map_or(Value::Undefined, |(_, column)| {
                                Value::Number(column as f64)
                            }),
                        ),
                    ])
                })
                .collect(),
        )))
    }

    pub(super) fn throw_error(
        &mut self,
        kind: ErrorKind,
        message: String,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let error = self.construct_error(kind, &[Value::String(message.into())]);
        self.unwind(error, instruction)
    }

    /// The value a failed call threw, for a built-in that has to keep going and report later.
    ///
    /// An error the machine raised carries a kind and a sentence rather than an object, so one
    /// is built here -- which is what a `catch` around the same call would have seen.
    pub(super) fn thrown_value(&mut self, error: ExecutionError) -> Result<Value, ExecutionError> {
        match error {
            ExecutionError::ThrownAcrossNative | ExecutionError::CaughtByHost => {
                Ok(self.caught.take().unwrap_or(Value::Undefined))
            }
            other => match other.as_javascript_error() {
                Some((kind, message)) => {
                    Ok(self.construct_error(kind, &[Value::String(message.into())]))
                }
                None => Err(other),
            },
        }
    }

    /// The error constructor a *program* reached, as opposed to one the machine raised itself.
    ///
    /// They differ in exactly one place. `new AggregateError(errors, message)` iterates its
    /// first argument -- it is `IterableToList`, not an array-like read -- so it can run the
    /// program's own `Symbol.iterator` and can fail. Every other kind takes its arguments as
    /// they are, which is why [`Vm::construct_error`] stays infallible and every internal
    /// `TypeError` the machine raises can keep using it.
    pub(super) fn construct_error_reached(
        &mut self,
        kind: ErrorKind,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if !matches!(kind, ErrorKind::Aggregate) {
            return Ok(self.construct_error(kind, arguments));
        }
        let source = arguments.first().cloned().unwrap_or(Value::Undefined);
        let errors = Value::Array(ArrayData::new(self.drain_iterable(&source, instruction)?));
        let mut listed = arguments.to_vec();
        match listed.first_mut() {
            Some(slot) => *slot = errors,
            None => listed.push(errors),
        }
        Ok(self.construct_error(kind, &listed))
    }

    /// Build an error instance whose prototype is this run's prototype for that kind.
    pub(super) fn construct_error(&mut self, kind: ErrorKind, arguments: &[Value]) -> Value {
        let prototype = self.error_prototype(kind);
        let error = build_error(kind, arguments);
        if let Value::Object(object) = &error {
            let mut object = object.borrow_mut();
            object.set_prototype(prototype);
            if let Some(accessors) = self.intrinsics.error_stack_accessors(self.current_realm) {
                object.properties.insert(
                    0,
                    Slot {
                        key: "stack".into(),
                        symbol: false,
                        internal: false,
                        property: Property::Accessor {
                            get: Some(accessors.getter),
                            set: Some(accessors.setter),
                        },
                        attributes: Attributes {
                            writable: false,
                            enumerable: false,
                            configurable: true,
                        },
                    },
                );
            }
            // Recorded where the error was *made*, not where it was reported: a rejection
            // surfaces after the stack that produced it is gone, and that is exactly the case
            // where a trace is worth having.
            object.set(
                ERROR_TRACE,
                Value::Array(ArrayData::new(
                    self.trace()
                        .into_iter()
                        .map(|value| Value::String(value.into()))
                        .collect(),
                )),
            );
            let stack_frames = self.error_stack_frame_limit().map(|limit| {
                let snapshot = |function: Option<&Rc<Closure>>,
                                method_name: Option<&StringUnits>,
                                program: &Program,
                                path: Option<&str>,
                                instruction: usize| {
                    let span = function
                        .and_then(|closure| program.functions.get(closure.function as usize))
                        .map(|proto| proto.span);
                    let site = program.debug_call_site(span, instruction);
                    ordinary_object([
                        (
                            "fileName".to_owned(),
                            Value::String(self.frame_file_name(path).into()),
                        ),
                        (
                            "functionName".to_owned(),
                            function
                                .and_then(|closure| Self::stack_function_name(closure.as_ref()))
                                .or_else(|| method_name.map(|name| name.to_lossy_string()))
                                .map_or(Value::Undefined, |name| Value::String(name.into())),
                        ),
                        (
                            "methodName".to_owned(),
                            method_name
                                .map_or(Value::Undefined, |name| Value::String(name.clone())),
                        ),
                        ("instruction".to_owned(), Value::Number(instruction as f64)),
                        (
                            "lineNumber".to_owned(),
                            site.map_or(Value::Undefined, |site| Value::Number(site.line as f64)),
                        ),
                        (
                            "columnNumber".to_owned(),
                            site.map_or(Value::Undefined, |site| Value::Number(site.column as f64)),
                        ),
                    ])
                };
                let mut frames = Vec::new();
                if !Self::idle_module_frame(
                    self.function.as_ref(),
                    &self.program,
                    self.instruction.saturating_sub(1),
                ) {
                    frames.push(snapshot(
                        self.function.as_ref(),
                        self.method_name.as_ref(),
                        &self.program,
                        self.module_path.as_deref(),
                        self.instruction.saturating_sub(1),
                    ));
                }
                for frame in self.frames.iter().rev() {
                    if Self::idle_module_frame(
                        frame.function.as_ref(),
                        &frame.program,
                        frame.instruction.saturating_sub(1),
                    ) {
                        continue;
                    }
                    frames.push(snapshot(
                        frame.function.as_ref(),
                        frame.method_name.as_ref(),
                        &frame.program,
                        frame.module_path.as_deref(),
                        frame.instruction.saturating_sub(1),
                    ));
                }
                frames.truncate(limit);
                Value::Array(ArrayData::new(frames))
            });
            object.error_stack_frames = stack_frames;
        }
        error
    }
}

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// Describe a value that is about to end the run as an uncaught throw.
    ///
    /// A host reports an uncaught value the way the program would print it, which for a
    /// user-defined error type means its own `toString`. Test262's whole assertion library throws
    /// `Test262Error`, whose `toString` is an ordinary function, so without this every failure in
    /// the suite reads `uncaught [object Object]` and the assertion message -- the only part that
    /// says what broke -- is lost.
    ///
    /// This runs user code, which is only acceptable because both callers are terminal: the run
    /// is ending either way. A `toString` that throws, or that answers something other than a
    /// string, falls back to the built-in description rather than replacing one uncaught error
    /// with another.
    pub(super) fn describe_uncaught(&mut self, thrown: &Value, instruction: usize) -> String {
        if has_error_to_string(thrown) {
            return error_to_string(thrown);
        }
        if matches!(
            thrown,
            Value::Object(_) | Value::Function(_) | Value::Array(_)
        ) && let Ok(Value::String(text)) =
            self.as_primitive(thrown, PrimitiveHint::String, instruction)
        {
            return text.to_lossy_string();
        }
        describe_thrown(thrown)
    }

    /// The `stack` an uncaught object carries, read as the program would read it.
    ///
    /// The native backend's launcher does exactly this, so the two reports agree. Like
    /// [`Self::describe_uncaught`] it runs user code (the lazy accessor, `prepareStackTrace`) and
    /// is only acceptable because the run is ending; a getter that throws or answers a
    /// non-string falls back to the short description.
    pub(super) fn uncaught_stack(&mut self, thrown: &Value, instruction: usize) -> Option<String> {
        if !matches!(thrown, Value::Object(_)) {
            return None;
        }
        match self.read_property(thrown, "stack", instruction) {
            Ok(Value::String(stack)) if !stack.is_empty() => Some(stack.to_lossy_string()),
            Ok(_) => None,
            Err(_) => {
                self.caught = None;
                None
            }
        }
    }

    /// Find the constructor name in a thrown value's prototype chain.
    ///
    /// This is the name Test262's negative contract compares, not the mutable `name` property
    /// used for display. The latter can be changed by a program, and primitives can be displayed
    /// with exactly the same spelling as an error. Starting at the thrown value itself means a
    /// program-supplied `constructor` property is observed just as `value.constructor` would be.
    pub(super) fn error_constructor(&self, thrown: &Value) -> Option<String> {
        let mut current = Some(thrown.clone());
        for _ in 0..MAX_PROTOTYPE_DEPTH {
            let value = current?;
            current = match &value {
                Value::Object(object) => {
                    let object = object.borrow();
                    if let Some(Property::Value(constructor)) =
                        object.get_own_property("constructor")
                    {
                        return constructor_name(constructor);
                    }
                    object.prototype.clone()
                }
                Value::Function(function) => {
                    let properties = function.properties.borrow();
                    if let Some(Property::Value(constructor)) =
                        properties.get_own_property("constructor")
                    {
                        return constructor_name(constructor);
                    }
                    properties.prototype.clone()
                }
                Value::Array(array) => {
                    let properties = array.properties.borrow();
                    if let Some(Property::Value(constructor)) =
                        properties.get_own_property("constructor")
                    {
                        return constructor_name(constructor);
                    }
                    properties.prototype.clone()
                }
                _ => return None,
            };
        }
        None
    }
}
