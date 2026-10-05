use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// Enter `function` with `argc` arguments sitting on top of the stack.
    ///
    /// Missing arguments become `undefined` and extra ones are discarded, which is what calling
    /// a JavaScript function with the wrong arity does. Neither is an error, so neither is
    /// reported as one.
    pub(super) fn call(
        &mut self,
        closure: Rc<Closure>,
        argc: u16,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        self.call_with_this(closure, argc, instruction, Value::Undefined, None, None)
    }

    /// The receiver sloppy code actually gets, which is never a primitive.
    ///
    /// Modern output rarely reaches this -- a module is strict, and TypeScript emits `"use
    /// strict"` -- but a CommonJS bundle is sloppy from its first line, and the idiom
    /// `var root = this;` at the top of a UMD wrapper is *reading the global* through exactly
    /// this rule.
    pub(super) fn bind_sloppy_this(&mut self, receiver: Value) -> Value {
        match receiver {
            Value::Undefined | Value::Null => self.global_object(),
            // The five boxable primitives, listed rather than derived: everything else -- a
            // function, an array, a proxy, a date, a typed array -- is already an object in the
            // specification's sense and `ToObject` hands it straight back.
            primitive @ (Value::String(_)
            | Value::Number(_)
            | Value::Boolean(_)
            | Value::Symbol(_)
            | Value::BigInt(_)) => wrap_primitive(primitive),
            other => other,
        }
    }

    /// Enter a closure with an explicit receiver.
    pub(super) fn call_with_this(
        &mut self,
        closure: Rc<Closure>,
        argc: u16,
        instruction: usize,
        this_value: Value,
        constructing: Option<Value>,
        method_name: Option<StringUnits>,
    ) -> Result<(), ExecutionError> {
        self.call_constructing(
            closure,
            argc,
            instruction,
            this_value,
            constructing,
            None,
            method_name,
        )
    }

    /// The same entry with `new.target` named rather than derived.
    ///
    /// `new C()` makes them the same function, which is why every other caller can leave this
    /// `None`. `Reflect.construct(C, [], D)` is the one operation that separates them: the body
    /// of `C` runs, and `new.target` inside it is `D`.
    pub(super) fn call_constructing(
        &mut self,
        closure: Rc<Closure>,
        argc: u16,
        instruction: usize,
        this_value: Value,
        constructing: Option<Value>,
        new_target: Option<Value>,
        method_name: Option<StringUnits>,
    ) -> Result<(), ExecutionError> {
        let function = closure.function;
        let Some(proto) = closure.program.functions.get(function as usize) else {
            return Err(ExecutionError::UnknownFunction { function });
        };
        // An arrow ignores the receiver entirely and uses what it captured. Resolved before the
        // generator branch because an async arrow is compiled to a generator, and it has to keep
        // the `this` it closed over rather than the `undefined` a bare call passes.
        let entering = match &closure.captured_this {
            Some(captured) => captured.clone(),
            None => this_value,
        };
        // `OrdinaryCallBindThis`: sloppy code never sees a primitive `this`.
        //
        // The specification substitutes the global object for `undefined` and `null` and
        // `ToObject`s everything else, so a sloppy `f.call(7)` receives a `Number` wrapper it can
        // write a property on and `f()` receives the global. Strict code receives exactly what it
        // was passed, which is what makes `this` `undefined` in a bare strict call.
        //
        // Skipped while constructing, where `this` is the new instance and already an object, and
        // for an arrow, which has no `this` of its own to bind -- both are settled above and
        // neither is a receiver this call chose.
        let entering =
            if closure.captured_this.is_none() && constructing.is_none() && !closure.is_strict() {
                self.bind_sloppy_this(entering)
            } else {
                entering
            };
        let target = match &closure.captured_new_target {
            Some(captured) => captured.clone(),
            None if constructing.is_some() => {
                new_target.unwrap_or_else(|| Value::Function(Rc::clone(&closure)))
            }
            None => Value::Undefined,
        };
        if proto.generator {
            // Calling a generator function does not run it: it captures the arguments and hands
            // back an object whose `next` runs the body up to each `yield`.
            // An async generator is called like a plain one: it hands back an object, and
            // nothing runs until the consumer asks for a step.
            let asynchronous = proto.asynchronous && !proto.async_generator;
            let generator = self.build_generator(&closure, argc, instruction, entering, target)?;
            if !asynchronous {
                self.stack.push(generator);
                return Ok(());
            }
            // An async function *does* run, up to its first `await`, and hands back a promise
            // rather than the generator -- which is the only difference the caller sees.
            let Value::Generator(generator) = generator else {
                unreachable!("`build_generator` produces a generator");
            };
            let promise = PromiseState::pending();
            self.drive_async(&generator, &promise, Resume::Send(Value::Undefined))?;
            self.stack.push(Value::Promise(promise));
            return Ok(());
        }
        if self.frames.len() >= self.limits.max_call_depth {
            return Err(ExecutionError::CallDepthExceeded {
                limit: self.limits.max_call_depth,
            });
        }

        let argc = argc as usize;
        if self.stack.len() < argc + 1 {
            return Err(ExecutionError::StackUnderflow { instruction });
        }
        let mut arguments = self.stack.split_off(self.stack.len() - argc);
        // The callee sits directly below its arguments; removing it here is what makes the
        // stack look, to the caller, as though the call left only its result.
        self.stack.pop();
        let received = arguments.clone();

        let mut locals = vec![Value::Undefined; proto.local_count as usize];
        arguments.truncate(proto.parameter_count as usize);
        for (slot, argument) in arguments.into_iter().enumerate() {
            locals[slot] = argument;
        }

        // `new.target` is the constructor this call named, which is this closure when the call
        // is a `new` and nothing otherwise. An arrow keeps the one it captured, for the same
        // reason it keeps `this`.
        // The frame keeps what this call was actually given, whatever the closure is: an arrow's
        // rest parameter needs it, and `LoadArguments` is the one place that substitutes the
        // captured list instead.
        self.frames.push(Frame {
            realm: std::mem::replace(&mut self.current_realm, closure.realm),
            program: std::mem::replace(&mut self.program, Rc::clone(&closure.program)),
            strictness: std::mem::replace(&mut self.strictness, Rc::clone(&closure.strictness)),
            strict: std::mem::replace(&mut self.strict, closure.is_strict()),
            module_path: std::mem::replace(&mut self.module_path, closure.module_path.clone()),
            module_generation: std::mem::replace(
                &mut self.module_generation,
                closure.module_generation,
            ),
            module_generation_owner: std::mem::replace(
                &mut self.module_generation_owner,
                closure
                    .module_generation_owner
                    .borrow()
                    .clone()
                    .or_else(|| {
                        self.module_map
                            .borrow()
                            .generation_handle(closure.realm, closure.module_generation)
                    }),
            ),
            function: self.function.take(),
            method_name: std::mem::replace(&mut self.method_name, method_name),
            this_value: std::mem::replace(&mut self.this_value, entering),
            new_target: std::mem::replace(&mut self.new_target, target),
            arguments: std::mem::replace(&mut self.arguments, received),
            arguments_object: self.arguments_object.take(),
            mapped_arguments: self.mapped_arguments,
            constructing,
            super_call: false,
            locals: std::mem::replace(&mut self.locals, locals),
            // Inherited: a called function reads the same module bindings its caller does.
            global_base: self.global_base,
            instruction: self.instruction,
            stack_base: self.stack.len(),
        });
        self.function = Some(closure);
        self.mapped_arguments = self
            .function
            .as_ref()
            .is_some_and(|closure| closure.has_mapped_arguments());
        self.instruction = 0;
        Ok(())
    }

    /// Compile source that arrived at run time and enter it as a frame of its own.
    ///
    /// An eval is a *program*, not a function: it has module-scope slots, and running it inside
    /// the caller's frame would put its bindings in the caller's numbered slots. So it gets a
    /// frame like a call does, and `global_base` moves with it so that its own top-level slots
    /// are what its nested functions call module scope.
    ///
    /// `site` is what makes it direct. With one, the caller's bindings arrive as cells the eval
    /// program binds by name, and its `this`, `arguments` and `new.target` are the caller's --
    /// which is the whole of what "evaluates in the caller's scope" means. Without one, this is
    /// an indirect eval: global scope, the global object as `this`, and no arguments.
    pub(super) fn enter_eval_units(
        &mut self,
        source: &StringUnits,
        site: Option<&EvalSite>,
        script: bool,
    ) -> Result<(), ExecutionError> {
        match source.to_scalar_cow() {
            Ok(text) => self.enter_eval(text.as_ref(), site, script),
            Err(_) => {
                let transport = source.to_lossy_string();
                self.enter_eval_with_units(&transport, Some(source), site, script)
            }
        }
    }

    pub(super) fn enter_eval(
        &mut self,
        source: &str,
        site: Option<&EvalSite>,
        script: bool,
    ) -> Result<(), ExecutionError> {
        self.enter_eval_with_units(source, None, site, script)
    }

    pub(super) fn enter_eval_with_units(
        &mut self,
        source: &str,
        source_units: Option<&StringUnits>,
        site: Option<&EvalSite>,
        script: bool,
    ) -> Result<(), ExecutionError> {
        let Some(compile) = self.source_compiler else {
            return Err(ExecutionError::NoSourceCompiler);
        };
        // Strict either because the caller is -- which only the compiler knew, so the site
        // carries it -- or because this source says so itself.
        let strict = site.is_some_and(|site| site.strict) || source_is_strict(source);
        let outer: Vec<kiln_ir::EvalOuter> = site
            .map(|site| {
                site.bindings
                    .iter()
                    .chain(site.with_objects.iter())
                    .map(|binding| kiln_ir::EvalOuter {
                        name: binding.name.clone(),
                        declaration: binding.declaration,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let with_objects: Vec<String> = site
            .map(|site| {
                site.with_objects
                    .iter()
                    .map(|binding| binding.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        // An indirect eval *is* global code, so its declarations always go on the global object.
        // A direct one goes there only when the call was written at the top level of a script,
        // which the compiler recorded on the site.
        let global_var_scope = site.is_none_or(|site| site.global_var_scope);
        // An indirect eval is global code with no method, no class and no call around it, so it
        // carries none of the syntactic context a direct one does.
        let new_target = site.map_or(kiln_ir::EvalNewTarget::Forbidden, |site| site.new_target);
        let no_private_scopes: [kiln_ir::PrivateScope; 0] = [];
        let eval_module_id = site.map_or_else(
            || "<indirect-eval>".to_owned(),
            |site| format!("{}#eval-{}", site.module_id, site.source_offset),
        );
        let (program, program_strictness) = compile(&EvalSource {
            source,
            source_units,
            module_id: &eval_module_id,
            outer: &outer,
            with_objects: &with_objects,
            strict,
            global_var_scope,
            script,
            home: site.and_then(|site| site.home.as_deref()),
            new_target,
            private_scopes: site.map_or(&no_private_scopes, |site| &site.private_scopes),
            // An indirect eval *is* global code: its variable environment is the global one and
            // there is no record between the two, so it never blocks a name.
            blocked_var_names: site.map_or(&[], |site| &site.blocked_var_names),
        })
        .map_err(|error| match error {
            EvalCompileError::Syntax(message) => ExecutionError::SyntaxErrorMessage { message },
            EvalCompileError::Unsupported(message) => ExecutionError::EvalUnsupported { message },
        })?;
        // `compile_eval` boxes every top-level binding precisely so this cannot happen. Checked
        // anyway, and refused rather than run: a `LoadGlobal` here would read the *caller's*
        // module frame by slot number and answer with whatever binding happens to share it,
        // which is the one failure mode an eval must never have.
        if reads_module_scope(&program) {
            return Err(ExecutionError::EvalUnsupported {
                message: "eval produced a program that reads module scope by slot".to_owned(),
            });
        }

        let mut locals = vec![Value::Undefined; program.local_count as usize];
        if let Some(site) = site {
            // Slot `i` for binding `i`, which is the contract `compile_eval` compiled against.
            for (slot, binding) in site.bindings.iter().enumerate() {
                let cell = match binding.access {
                    EvalAccess::Cell(slot) => self.cell(slot as usize)?,
                    EvalAccess::Upvalue(index) => self.upvalue(index)?,
                };
                let readonly_kind = match binding.declaration {
                    kiln_ir::EvalDeclaration::Const => Some(crate::vm::ReadonlyCellKind::Const),
                    kiln_ir::EvalDeclaration::FunctionName => {
                        Some(crate::vm::ReadonlyCellKind::FunctionName)
                    }
                    kiln_ir::EvalDeclaration::Var | kiln_ir::EvalDeclaration::Let => None,
                };
                if let Some(kind) = readonly_kind {
                    let initializer_program =
                        (kind == crate::vm::ReadonlyCellKind::Const).then_some(&self.program);
                    self.readonly_cells
                        .borrow_mut()
                        .mark(&cell, kind, initializer_program);
                }
                locals[slot] = Value::Cell(cell);
            }
            for (offset, binding) in site.with_objects.iter().enumerate() {
                let cell = match binding.access {
                    EvalAccess::Cell(slot) => self.cell(slot as usize)?,
                    EvalAccess::Upvalue(index) => self.upvalue(index)?,
                };
                locals[site.bindings.len() + offset] = Value::Cell(cell);
            }
        }

        if self.frames.len() >= self.limits.max_call_depth {
            return Err(ExecutionError::CallDepthExceeded {
                limit: self.limits.max_call_depth,
            });
        }
        let (this_value, arguments, frame_new_target) = match site {
            Some(_) => (
                self.this_value.clone(),
                self.arguments.clone(),
                // A class field initialiser is its own function boundary for `new.target`, and
                // Kiln emits its code into the constructor -- so the running frame's value is
                // the constructor's, and an eval written in an initialiser must not see it.
                match new_target {
                    kiln_ir::EvalNewTarget::Undefined => Value::Undefined,
                    kiln_ir::EvalNewTarget::Forbidden | kiln_ir::EvalNewTarget::Caller => {
                        self.new_target.clone()
                    }
                },
            ),
            // An indirect eval is called with no receiver from global scope, which makes `this`
            // the global object -- the one place a sloppy call still gets one.
            None => (self.global_object(), Vec::new(), Value::Undefined),
        };
        let program_is_strict = program_strictness.body;
        self.frames.push(Frame {
            realm: self.current_realm,
            program: std::mem::replace(&mut self.program, Rc::new(program)),
            strictness: std::mem::replace(&mut self.strictness, Rc::new(program_strictness)),
            strict: std::mem::replace(&mut self.strict, program_is_strict),
            module_path: self.module_path.clone(),
            module_generation: self.module_generation,
            module_generation_owner: self.module_generation_owner.clone(),
            function: self.function.take(),
            method_name: std::mem::replace(&mut self.method_name, None),
            this_value: std::mem::replace(&mut self.this_value, this_value),
            new_target: std::mem::replace(&mut self.new_target, frame_new_target),
            arguments: std::mem::replace(&mut self.arguments, arguments),
            arguments_object: self.arguments_object.take(),
            mapped_arguments: self.mapped_arguments,
            constructing: None,
            super_call: false,
            locals: std::mem::replace(&mut self.locals, locals),
            global_base: self.global_base,
            instruction: self.instruction,
            stack_base: self.stack.len(),
        });
        // A direct eval evaluates in the caller's scope, so its `arguments` is the caller's --
        // the same object, of the same kind. An indirect one is global code with no call around
        // it and no `arguments` at all.
        self.mapped_arguments = site.is_some() && self.mapped_arguments;
        // The eval program is its own module scope from here until its frame returns. It is not
        // in `frames` yet -- its locals are the running ones -- and the index it *will* occupy
        // when it makes a call of its own is exactly the depth the stack has now.
        self.global_base = self.frames.len();
        self.instruction = 0;
        Ok(())
    }

    /// Run an eval to completion from inside a built-in, and hand back its value.
    ///
    /// The instruction path does not need this: it enters the frame and lets the interpreter
    /// loop carry on. A built-in cannot, because it owes its caller a value now -- so it drives
    /// the machine the way every other built-in that calls back into JavaScript does, and pays
    /// the same Rust frame for it.
    pub(super) fn drive_eval_units(
        &mut self,
        source: &StringUnits,
        site: Option<&EvalSite>,
        script: bool,
    ) -> Result<Value, ExecutionError> {
        match source.to_scalar_cow() {
            Ok(text) => self.drive_eval(text.as_ref(), site, script),
            Err(_) => {
                let transport = source.to_lossy_string();
                self.drive_eval_with_units(&transport, Some(source), site, script)
            }
        }
    }

    pub(super) fn drive_eval(
        &mut self,
        source: &str,
        site: Option<&EvalSite>,
        script: bool,
    ) -> Result<Value, ExecutionError> {
        self.drive_eval_with_units(source, None, site, script)
    }

    fn drive_eval_with_units(
        &mut self,
        source: &str,
        source_units: Option<&StringUnits>,
        site: Option<&EvalSite>,
        script: bool,
    ) -> Result<Value, ExecutionError> {
        if self.native_depth >= self.limits.max_native_depth {
            return Err(ExecutionError::CallDepthExceeded {
                limit: self.limits.max_native_depth,
            });
        }
        self.native_depth += 1;
        let depth = self.frames.len();
        let outcome = self
            .enter_eval_with_units(source, source_units, site, script)
            .and_then(|()| self.run_frames(depth).map(|result| result.value));
        self.native_depth -= 1;
        outcome
    }

    /// `eval(x)` reached as a value: an indirect eval, which evaluates in global scope.
    ///
    /// A non-string argument comes straight back. That is not a shortcut -- the specification
    /// says an eval of anything but a string *is* that value, which is what makes
    /// `eval(someObject)` harmless.
    pub(super) fn indirect_eval(
        &mut self,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        match arguments.first() {
            Some(Value::String(source)) => {
                if !self.string_code_generation_allowed {
                    self.throw_error(
                        ErrorKind::Eval,
                        "Code generation from strings disallowed for this context".into(),
                        instruction,
                    )?;
                    return Ok(Value::Undefined);
                }
                self.drive_eval_units(source, None, false)
            }
            Some(value) => Ok(value.clone()),
            None => Ok(Value::Undefined),
        }
    }

    /// `new Function(a, b, "return a + b")`, which is an eval of an assembled function literal.
    ///
    /// The specification builds source text and evaluates it, and so does this: the parameters
    /// are joined with commas and the body is the last argument, both `ToString`-ed by ordinary
    /// coercion. Parenthesised so that the program's completion value is the function rather
    /// than a declaration that binds it somewhere and answers `undefined`.
    ///
    /// Global scope, never the caller's: `Function` is reached as a value, and a value carries
    /// no scope. That is the difference between it and a direct `eval`, and it is why a function
    /// built this way cannot see the locals around the call that built it.
    pub(super) fn function_from_source(
        &mut self,
        arguments: &[Value],
        instruction: usize,
        kind: &str,
    ) -> Result<Value, ExecutionError> {
        // Every parameter first, in order, and only then the body. That order is the
        // specification's and it is observable: when both a parameter's `toString` and the
        // body's throw, the parameter's throw is the one the program sees. Keep each result in
        // UTF-16 units until the one checked parser boundary below.
        let mut source = StringUnitsBuilder::new();
        source.push_ascii("(")?;
        source.push_ascii(kind)?;
        source.push_ascii(" anonymous(")?;
        if let Some((body, parameters)) = arguments.split_last() {
            for (index, parameter) in parameters.iter().enumerate() {
                if index > 0 {
                    source.push_ascii(",")?;
                }
                source.push_units(&self.as_text_units(parameter, instruction)?)?;
            }
            // The newlines are the specification's own: they stop a `//` comment in the
            // parameter list from swallowing the brace, and one in the body from swallowing the
            // close.
            source.push_ascii("\n) {\n")?;
            source.push_units(&self.as_text_units(body, instruction)?)?;
            source.push_ascii("\n})")?;
        } else {
            source.push_ascii("\n) {\n\n})")?;
        }
        let source = source.finish();
        if !self.string_code_generation_allowed {
            self.throw_error(
                ErrorKind::Eval,
                "Code generation from strings disallowed for this context".into(),
                instruction,
            )?;
            return Ok(Value::Undefined);
        }
        self.drive_eval_units(&source, None, false)
    }

    /// `ToString`: the conversion the *program* defines, which runs its code and can throw.
    ///
    /// Named `as_` rather than `to_` for the reason [`Vm::as_primitive`] is: it takes `&mut self`
    /// because converting an object *calls* one of its methods.
    ///
    /// This is not [`Value::to_display_string`], and the distinction is not cosmetic. That one is
    /// the host's own spelling of a value -- what the console prints -- and it answers
    /// `"[object Object]"` for any plain object without consulting it. Where the specification
    /// says `ToString`, using it silently skips the program's `toString`: `new Function` built
    /// `function anonymous([object Object]) {}` and died on a parse error inside a string the
    /// program never wrote, instead of either calling the `toString` that was there or
    /// propagating what it threw.
    pub(super) fn as_string(
        &mut self,
        value: &Value,
        instruction: usize,
    ) -> Result<String, ExecutionError> {
        Ok(self.as_text_units(value, instruction)?.to_lossy_string())
    }
}
