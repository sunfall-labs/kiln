use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// Pop `argc` arguments, leaving whatever is beneath them.
    pub(super) fn take_call_arguments(
        &mut self,
        argc: u16,
        instruction: usize,
    ) -> Result<Vec<Value>, ExecutionError> {
        let argc = argc as usize;
        if self.stack.len() < argc {
            return Err(ExecutionError::StackUnderflow { instruction });
        }
        Ok(self.stack.split_off(self.stack.len() - argc))
    }

    /// Invoke `method` with an explicit receiver.
    ///
    /// Returns `None` when a frame was pushed instead: a compiled function's result arrives via
    /// `Return`, so pushing anything here would leave a stray value on the caller's stack.
    pub(super) fn invoke(
        &mut self,
        mut method: Value,
        mut receiver: Value,
        mut arguments: Vec<Value>,
        instruction: usize,
        method_name: Option<StringUnits>,
    ) -> Result<Option<Value>, ExecutionError> {
        let mut forwarded_call = false;
        // Resolve `.call` and `.apply` before entering the final callee. A compiled target
        // pushes a frame whose realm must stay current until that frame returns; entering it
        // through a temporary native-method scope would restore the old realm too early.
        while let Value::Native(native @ (Native::FunctionCall | Native::FunctionApply), _) =
            &method
        {
            forwarded_call = true;
            let realm = value_realm(&method);
            let (this_value, forwarded) = self.entering(realm, |vm| {
                vm.forwarded_function_arguments(*native, &arguments, instruction)
            })?;
            method = receiver;
            receiver = this_value;
            arguments = forwarded;
        }
        if forwarded_call && matches!(&method, Value::Function(_)) {
            // `call_with_this` records the caller's realm in the saved frame and installs the
            // closure's realm. The run loop restores the saved realm on `Return`.
            return self.invoke_dispatch(method, receiver, arguments, instruction, method_name);
        }
        if forwarded_call {
            // Native built-ins, proxies and bound functions may call into JavaScript before
            // returning. Their ordinary call path owns that immediate result and its nested
            // frame lifetime; sending them through the frame dispatcher changes that contract.
            return self
                .call_value(&method, receiver, arguments, instruction)
                .map(Some);
        }
        let realm = value_realm(&method);
        self.entering(realm, |vm| {
            vm.invoke_dispatch(method, receiver, arguments, instruction, method_name)
        })
    }

    pub(super) fn invoke_dispatch(
        &mut self,
        method: Value,
        receiver: Value,
        arguments: Vec<Value>,
        instruction: usize,
        method_name: Option<StringUnits>,
    ) -> Result<Option<Value>, ExecutionError> {
        if matches!(
            &method,
            Value::Native(
                Native::AsyncFunctionPrototype
                    | Native::GeneratorFunctionPrototype
                    | Native::AsyncGeneratorFunctionPrototype,
                _
            )
        ) {
            return Err(ExecutionError::NotCallable {
                instruction,
                value: method.type_name(),
            });
        }
        match method {
            // The same rule as in `call_value`: a mutating array method given a proxy has to
            // write through it, not into a snapshot.
            Value::Native(native, _)
                if native.mutates_an_array() && matches!(receiver, Value::Proxy(_)) =>
            {
                self.mutate_through_proxy(native, &receiver, &arguments, instruction)
                    .map(Some)
            }
            Value::Native(native, _) => {
                // A typed array reached through one of the generic array methods walks as its
                // elements, the same coercion `call_value` applies to a redirected one. The two
                // named here do *not* walk a snapshot and are handed back untouched; they come
                // through only so the detached-buffer check inside sees them, which is the one
                // place every `%TypedArray%.prototype` method can be caught at once.
                let original = receiver.clone();
                let receiver = if native.walks_an_array()
                    || matches!(native, Native::ArrayJoinDefault | Native::ArrayCopyWithin)
                {
                    self.as_array_like(
                        &Value::Native(native, RealmId::PRIMARY),
                        receiver,
                        instruction,
                    )?
                } else {
                    receiver
                };
                let produced = self.dispatch_native(native, &receiver, &arguments, instruction)?;
                Ok(Some(self.typed_result(
                    &original,
                    produced,
                    native.rearranges_in_place(),
                )?))
            }
            // `next` on an async generator answers with a promise rather than a result: the
            // body may await any number of times before it reaches the value the consumer asked
            // for, and none of that can happen inside this call.
            Value::Callback(CallbackMethod::AsyncGeneratorNext, _) => {
                let Value::Generator(generator) = &receiver else {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: receiver.type_name(),
                    });
                };
                let generator = Rc::clone(generator);
                let sent = arguments.first().cloned().unwrap_or(Value::Undefined);
                let step = PromiseState::pending();
                self.drive_async_generator(&generator, &step, Resume::Send(sent))?;
                Ok(Some(Value::Promise(step)))
            }
            // `return` and `throw` on an async generator, both of which finish -- or unwind --
            // the body and answer with a promise of the resulting step. The two share a path
            // because the only thing that differs is which abrupt completion is delivered.
            Value::Callback(
                method @ (CallbackMethod::AsyncGeneratorReturn
                | CallbackMethod::AsyncGeneratorThrow),
                _,
            ) => {
                let Value::Generator(generator) = &receiver else {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: receiver.type_name(),
                    });
                };
                let generator = Rc::clone(generator);
                let sent = arguments.first().cloned().unwrap_or(Value::Undefined);
                let step = PromiseState::pending();
                let returning = matches!(method, CallbackMethod::AsyncGeneratorReturn);
                let entered = {
                    let state = generator.borrow();
                    state.started && !state.done
                };
                if entered {
                    let resume = if returning {
                        Resume::Return(sent)
                    } else {
                        Resume::Throw(sent)
                    };
                    self.drive_async_generator(&generator, &step, resume)?;
                } else {
                    // Never entered, or already finished: there is nothing to unwind, so the
                    // completion is answered directly. `return` still awaits its argument --
                    // `AsyncGeneratorAwaitReturn` does -- which is observable as a tick and as a
                    // rejection when the argument is a rejected promise.
                    generator.borrow_mut().complete();
                    if returning {
                        let target = match sent {
                            Value::Promise(inner) => inner,
                            other => {
                                let pending = PromiseState::pending();
                                self.resolve_promise(&pending, other);
                                pending
                            }
                        };
                        self.react(
                            &target,
                            Value::Undefined,
                            Value::Undefined,
                            Sink::AsyncGeneratorReturn(Rc::clone(&step)),
                            false,
                        );
                    } else {
                        self.settle(&step, &Settled::Rejected(sent));
                    }
                }
                Ok(Some(Value::Promise(step)))
            }
            // Generator methods drive the machine, so they cannot go through `array_callback`.
            Value::Callback(CallbackMethod::GeneratorThrow, _) => {
                let Value::Generator(generator) = &receiver else {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: receiver.type_name(),
                    });
                };
                let generator = Rc::clone(generator);
                let thrown = arguments.first().cloned().unwrap_or(Value::Undefined);
                let entered = {
                    let state = generator.borrow();
                    state.started && !state.done
                };
                if !entered {
                    // A generator that never started, or that has finished, has no `try` to
                    // catch this: the specification marks it completed and rethrows at the
                    // caller, which is where the value came from.
                    generator.borrow_mut().complete();
                    self.caught = Some(thrown);
                    return Err(ExecutionError::ThrownAcrossNative);
                }
                let delegate = generator.borrow().delegate.clone();
                let (value, done) = if let Some((delegate_iter, exit_target, _wrapped)) = delegate {
                    self.delegate_generator_throw(
                        &generator,
                        delegate_iter,
                        exit_target,
                        thrown,
                        instruction,
                    )?
                } else {
                    self.resume_generator_with(&generator, Resume::Throw(thrown), instruction)?
                };
                if generator.borrow().delegate.is_some() && !done {
                    Ok(Some(value))
                } else {
                    Ok(Some(iteration_result(value, done)))
                }
            }
            Value::Callback(
                method @ (CallbackMethod::GeneratorNext | CallbackMethod::GeneratorReturn),
                _,
            ) => {
                let Value::Generator(generator) = &receiver else {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: receiver.type_name(),
                    });
                };
                let generator = Rc::clone(generator);
                let sent = arguments.first().cloned().unwrap_or(Value::Undefined);
                let (value, done) = if matches!(method, CallbackMethod::GeneratorReturn) {
                    let started = generator.borrow().started;
                    if started {
                        let delegate = generator.borrow().delegate.clone();
                        if let Some((delegate_iter, exit_target, _wrapped)) = delegate {
                            self.delegate_generator_return(
                                &generator,
                                delegate_iter,
                                exit_target,
                                sent,
                                instruction,
                            )?
                        } else {
                            // A started generator has to unwind: every `finally` between the
                            // suspension point and the top runs before it is done.
                            self.resume_generator_return(&generator, sent, instruction)?
                        }
                    } else {
                        // Never entered, so there is nothing to unwind.
                        generator.borrow_mut().complete();
                        (sent, true)
                    }
                } else {
                    generator.borrow_mut().delegate = None;
                    self.resume_generator(&generator, sent, instruction)?
                };
                if generator.borrow().delegate.is_some() && !done {
                    Ok(Some(value))
                } else {
                    Ok(Some(iteration_result(value, done)))
                }
            }
            // `reduce` threads an accumulator and `sort` compares pairs, so neither is the walk
            // the other callback methods share. All three coerce their receiver first, for the
            // same reason the native array methods do.
            Value::Callback(CallbackMethod::Reduce | CallbackMethod::ReduceRight, _) => {
                let backwards = matches!(method, Value::Callback(CallbackMethod::ReduceRight, _));
                let receiver = self.as_array_like(&method, receiver, instruction)?;
                self.array_reduce(&receiver, &arguments, backwards, instruction)
                    .map(Some)
            }
            Value::Callback(
                callback @ (CallbackMethod::FindLast | CallbackMethod::FindLastIndex),
                _,
            ) => {
                let receiver = self.as_array_like(&method, receiver, instruction)?;
                self.array_callback_backwards(callback, &receiver, &arguments, instruction)
                    .map(Some)
            }
            Value::Callback(CallbackMethod::Sort, _) => {
                let original = receiver.clone();
                let receiver = self.as_array_like(&method, receiver, instruction)?;
                let produced = self.array_sort(&receiver, &arguments, instruction)?;
                Ok(Some(self.typed_result(&original, produced, true)?))
            }
            Value::Callback(callback, _) => {
                let original = receiver.clone();
                let receiver = self.as_array_like(&method, receiver, instruction)?;
                let produced = self.array_callback(callback, &receiver, &arguments, instruction)?;
                Ok(Some(self.typed_result(&original, produced, false)?))
            }
            // A bound function ignores the receiver the call site offers, whichever way it is
            // reached, so both go through one path.
            // A wrapped function ignores it too, and for a stronger reason: a receiver would be
            // one more object crossing the boundary, which the wrapping does not permit.
            // A proxy keeps the receiver but must run its `apply` trap (or its target), which
            // the ordinary call path owns.
            value @ Value::Proxy(_) => self
                .call_value(&value, receiver, arguments, instruction)
                .map(Some),
            value if forwards_to_call_value(&value) => self
                .call_value(&value, receiver, arguments, instruction)
                .map(Some),
            Value::Function(closure) => {
                // The receiver becomes `this`. Pushing a placeholder plus the arguments is how
                // the shared call path receives them; it pops the placeholder as the callee.
                self.stack.push(Value::Undefined);
                let argc = arguments.len();
                for argument in arguments {
                    self.stack.push(argument);
                }
                self.call_with_this(
                    closure,
                    argc as u16,
                    instruction,
                    receiver,
                    None,
                    method_name,
                )?;
                Ok(None)
            }
            other => Err(ExecutionError::NotCallable {
                instruction,
                value: other.type_name(),
            }),
        }
    }

    /// Pop `argc` arguments and the callee beneath them.
    pub(super) fn take_arguments(
        &mut self,
        argc: u16,
        instruction: usize,
    ) -> Result<Vec<Value>, ExecutionError> {
        let argc = argc as usize;
        if self.stack.len() < argc + 1 {
            return Err(ExecutionError::StackUnderflow { instruction });
        }
        let arguments = self.stack.split_off(self.stack.len() - argc);
        self.stack.pop();
        Ok(arguments)
    }

    /// `object[key]`. Array indices are numeric; everything else is a string key.
    /// Whether an array slot is a hole rather than a value.
    pub(super) fn is_hole(value: &Value) -> bool {
        matches!(value, Value::Native(Native::Hole, _))
    }

    /// `Map.prototype.getOrInsert` and its three siblings: read a key, and write it if it was
    /// not there.
    ///
    /// The point of the pair is that they look the key up *once*. `m.has(k) || m.set(k, v)` is
    /// two walks and a race with anything the intervening code does, and the computed spelling
    /// is the one that cannot be written correctly outside: its callback runs only on a miss and
    /// may itself touch the map, so the key is looked for again afterwards rather than the
    /// earlier answer being trusted.
    pub(super) fn collection_get_or_insert(
        &mut self,
        weak: bool,
        computed: bool,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let native = Native::CollectionGetOrInsert { weak, computed };
        if weak {
            validate_weak_map_receiver(receiver)?;
            let key = arguments.first().cloned().unwrap_or(Value::Undefined);
            if !self.can_be_held_weakly(&key) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Invalid value used as weak map key".to_owned(),
                });
            }
            let key_slot =
                WeakCollectionKey::new(&key).ok_or_else(|| ExecutionError::TypeErrorMessage {
                    message: "Invalid value used as weak map key".to_owned(),
                })?;
            let supplied = arguments.get(1).cloned().unwrap_or(Value::Undefined);
            if computed && !is_callable(&supplied) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "getOrInsertComputed: callbackfn must be a function".to_owned(),
                });
            }
            if let Some(value) = weak_map_value(receiver, &key) {
                return Ok(value);
            }
            let value = if computed {
                self.call_value(&supplied, Value::Undefined, vec![key.clone()], instruction)?
            } else {
                supplied
            };
            set_weak_map_value(receiver, &key, &key_slot, value.clone())?;
            return Ok(value);
        }
        let (entries, is_map) = require_collection(receiver, native)?;
        let key = canonical_key(arguments.first().cloned().unwrap_or(Value::Undefined));
        if weak && !self.can_be_held_weakly(&key) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Invalid value used as weak map key".to_owned(),
            });
        }
        let supplied = arguments.get(1).cloned().unwrap_or(Value::Undefined);
        if computed && !is_callable(&supplied) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "getOrInsertComputed: callbackfn must be a function".to_owned(),
            });
        }
        if let Some(found) = Self::collection_value(&entries, &key, is_map) {
            return Ok(found);
        }
        let value = if computed {
            self.call_value(&supplied, Value::Undefined, vec![key.clone()], instruction)?
        } else {
            supplied
        };
        // The callback may have inserted the key itself, or emptied the map, or both. What the
        // specification does here is look again rather than trust the miss above -- so a
        // callback that inserted the key sees its entry *updated* instead of duplicated.
        if let Some(index) = find_collection_entry(&entries, &key, is_map) {
            if let Some(Value::Array(pair)) = entries.borrow().get(index) {
                pair.borrow_mut()[1] = value.clone();
            }
            return Ok(value);
        }
        let new_idx = entries.borrow().len();
        entries.borrow_mut().push(Value::Array(ArrayData::new(vec![
            key.clone(),
            value.clone(),
        ])));
        collection_entry_inserted(&entries, &key, new_idx);
        bump_size(receiver, 1.0);
        Ok(value)
    }

    /// The value a map holds under `key`, if it holds one.
    pub(super) fn collection_value(
        entries: &Rc<ArrayData>,
        key: &Value,
        is_map: bool,
    ) -> Option<Value> {
        let index = find_collection_entry(entries, key, is_map)?;
        match entries.borrow().get(index) {
            Some(Value::Array(pair)) => {
                Some(pair.borrow().get(1).cloned().unwrap_or(Value::Undefined))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod weak_get_or_insert_tests {
    use super::*;
    use kiln_host_abi::{HostError, HostOp};

    struct Host;

    impl HostOps for Host {
        fn call(
            &mut self,
            _operation: HostOp,
            _arguments: &[HostValue],
        ) -> Result<HostValue, HostError> {
            Ok(HostValue::Undefined)
        }
    }

    #[test]
    fn weak_map_receiver_is_checked_before_computed_callback_runs() {
        let counter = Rc::new(RefCell::new(Value::Number(0.0)));
        let callback_program = Rc::new(Program {
            source: "".into(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: vec![Constant::Number(1.0f64.to_bits())],
            code: Vec::new(),
            local_count: 0,
            functions: vec![kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: None,
                arrow: false,
                generator: false,
                async_generator: false,
                is_method: false,
                is_class: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 0,
                code: vec![
                    Instruction::LoadUpvalue(0),
                    Instruction::Const(0),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::StoreUpvalue(0),
                    Instruction::LoadUpvalue(0),
                    Instruction::Return,
                ],
                upvalues: Vec::new(),
                type_annotations: None,
            }],
        });
        let callback = Value::Function(Rc::new(Closure {
            program: Rc::clone(&callback_program),
            function: 0,
            strictness: Rc::new(Strictness::default()),
            module_path: None,
            module_generation: 0,
            module_generation_owner: RefCell::new(None),
            name: None,
            inferred_name: None,
            upvalues: vec![Rc::clone(&counter)],
            realm: RealmId::PRIMARY,
            captured_arguments: None,
            captured_new_target: None,
            captured_this: None,
            prototype: RefCell::new(None),
            properties: RefCell::new(ObjectData::default()),
        }));
        let empty_program = Rc::new(Program {
            source: "".into(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: Vec::new(),
            code: Vec::new(),
            local_count: 0,
            functions: Vec::new(),
        });
        let mut host = Host;
        let mut vm = Vm::with_entropy_seed(
            empty_program,
            Rc::new(Strictness::default()),
            &mut host,
            Vec::new(),
            ExecutionLimits::default(),
            &RealmCapabilities::default(),
            ExecutionCapabilities::default(),
        )
        .unwrap();

        let result = vm.collection_get_or_insert(
            true,
            true,
            &Value::Undefined,
            &[
                Value::Object(Rc::new(RefCell::new(ObjectData::default()))),
                callback,
            ],
            0,
        );

        assert!(matches!(
            result,
            Err(ExecutionError::TypeErrorMessage { message })
                if message == "incompatible WeakMap receiver"
        ));
        assert!(matches!(*counter.borrow(), Value::Number(0.0)));
    }
}

fn weak_map_value(receiver: &Value, key: &Value) -> Option<Value> {
    let Value::Object(object) = receiver else {
        return None;
    };
    let object = object.borrow();
    if !matches!(object.get_own("@@map"), Some(&Value::Boolean(true)))
        || object.weak_collection_entries.is_none()
    {
        return None;
    }
    object
        .weak_collection_entries
        .as_ref()?
        .iter()
        .find(|entry| entry.key.upgrade().as_ref() == Some(key))
        .map(|entry| entry.value.clone())
}

fn validate_weak_map_receiver(receiver: &Value) -> Result<(), ExecutionError> {
    let Value::Object(object) = receiver else {
        return Err(ExecutionError::TypeErrorMessage {
            message: "incompatible WeakMap receiver".to_owned(),
        });
    };
    let object = object.borrow();
    if !matches!(object.get_own("@@map"), Some(&Value::Boolean(true)))
        || object.weak_collection_entries.is_none()
    {
        return Err(ExecutionError::TypeErrorMessage {
            message: "incompatible WeakMap receiver".to_owned(),
        });
    }
    Ok(())
}

fn set_weak_map_value(
    receiver: &Value,
    key: &Value,
    weak_key: &WeakCollectionKey,
    value: Value,
) -> Result<(), ExecutionError> {
    let Value::Object(object) = receiver else {
        return Err(ExecutionError::TypeErrorMessage {
            message: "incompatible WeakMap receiver".to_owned(),
        });
    };
    let mut object = object.borrow_mut();
    if !matches!(object.get_own("@@map"), Some(&Value::Boolean(true))) {
        return Err(ExecutionError::TypeErrorMessage {
            message: "incompatible WeakMap receiver".to_owned(),
        });
    }
    let Some(entries) = object.weak_collection_entries.as_mut() else {
        return Err(ExecutionError::TypeErrorMessage {
            message: "incompatible WeakMap receiver".to_owned(),
        });
    };
    if let Some(entry) = entries
        .iter_mut()
        .find(|entry| entry.key.upgrade().as_ref() == Some(key))
    {
        entry.value = value;
    } else {
        entries.push(WeakCollectionEntry {
            key: weak_key.clone(),
            value,
        });
    }
    Ok(())
}
