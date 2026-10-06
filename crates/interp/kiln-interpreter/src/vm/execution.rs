use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    fn write_cell(
        &mut self,
        cell: &Cell,
        value: Value,
        instruction: usize,
        initializing_store: bool,
    ) -> Result<bool, ExecutionError> {
        let program = Rc::clone(&self.program);
        let readonly =
            self.readonly_cells
                .borrow_mut()
                .kind_for_write(cell, &program, initializing_store);
        if let Some(kind) = readonly {
            if kind == ReadonlyCellKind::Const
                && matches!(&*cell.borrow(), Value::Native(Native::Uninitialized, _))
            {
                self.throw_error(
                    ErrorKind::Reference,
                    "cannot access a binding before its declaration".to_owned(),
                    instruction,
                )?;
            } else if self.strict || kind == ReadonlyCellKind::Const {
                self.throw_error(
                    ErrorKind::Type,
                    "Assignment to constant variable.".to_owned(),
                    instruction,
                )?;
            }
            // A named function expression's private name ignores sloppy writes and throws in
            // strict code. Const writes throw in both modes above.
            return Ok(true);
        }
        *cell.borrow_mut() = value;
        Ok(false)
    }

    /// The dispatch loop reached [`Self::checkpoint`]: find out which boundary that was.
    ///
    /// Deliberately not inlined into the loop. Everything here is either the end of the program
    /// or the end of a turn, and both are rare next to the instruction that got here.
    #[cold]
    pub(crate) fn reach_checkpoint(&mut self) -> Result<(), ExecutionError> {
        if self.steps >= self.limits.max_steps {
            return Err(ExecutionError::StepLimitExceeded {
                steps: self.steps,
                limit: self.limits.max_steps,
            });
        }
        // A worker whose parent called `terminate()` stops here, wherever it was: node stops a
        // worker in the middle of a loop, not only once it is idle.
        if self.in_worker && kiln_runtime::workers::terminate_requested() {
            return Err(ExecutionError::ProcessExited(1));
        }
        // A quantum, then. Hand the baton on and take it back before the next instruction runs:
        // this agent's Rust stack, frames and operand stack all stay exactly where they are,
        // which is the whole reason an agent is a thread rather than a continuation.
        if let Some(group) = &self.agents
            && group.scheduled()
        {
            let group = Arc::clone(group);
            group
                .tick(self.agent_id)
                .map_err(|_| ExecutionError::AgentTerminated)?;
        }
        self.checkpoint = self.next_checkpoint();
        // Whatever it settled queues a promise reaction, which the ordinary microtask checkpoint
        // takes: settling is not itself a checkpoint.
        self.settle_async_waits()?;
        Ok(())
    }

    /// Where the dispatch loop should stop next.
    pub(crate) fn next_checkpoint(&self) -> u64 {
        match &self.agents {
            Some(group) if group.scheduled() => self
                .steps
                .saturating_add(agent::QUANTUM_STEPS)
                .min(self.limits.max_steps),
            _ if self.in_worker => self
                .steps
                .saturating_add(crate::vm::WORKER_QUANTUM)
                .min(self.limits.max_steps),
            _ => self.limits.max_steps,
        }
    }

    pub(crate) fn run(&mut self) -> Result<ExecutionResult, ExecutionError> {
        // `usize::MAX` never matches a real frame depth, so the loop ends only at the outermost
        // `Return` -- the end of the program.
        self.run_frames(usize::MAX)
    }

    /// Run until the frame stack drops back to `stop_at`, or until the program ends.
    ///
    /// The boundary is what lets a built-in call back into user code: `map` pushes a frame for
    /// its callback and then drives the machine until that frame returns, rather than needing a
    /// second interpreter or a continuation.
    pub(crate) fn run_frames(&mut self, stop_at: usize) -> Result<ExecutionResult, ExecutionError> {
        // `usize::MAX` is an instruction pointer no frame can reach, so the second boundary
        // never fires for an ordinary run.
        self.run_bounded(stop_at, usize::MAX, 0)
    }

    /// As [`Self::run_frames`], stopping also when frame `stop_depth` reaches `stop_before`.
    ///
    /// The second boundary is a *position* rather than a return, which is what a generator's
    /// parameter binding needs: it has to run and then stop with its frame intact, because the
    /// locals it just wrote are the generator's and a `Return` would discard them.
    pub(crate) fn run_bounded(
        &mut self,
        stop_at: usize,
        stop_before: usize,
        stop_depth: usize,
    ) -> Result<ExecutionResult, ExecutionError> {
        let run_guard = crate::gc_census::enter_vm_run();
        self.run_bounded_inner(stop_at, stop_before, stop_depth, run_guard.outermost())
    }

    fn run_bounded_inner(
        &mut self,
        stop_at: usize,
        stop_before: usize,
        stop_depth: usize,
        outermost_run: bool,
    ) -> Result<ExecutionResult, ExecutionError> {
        // Held across steps and refreshed only when the running frame's program actually
        // changes. The borrow has to come from a local rather than from `self`, which every
        // instruction below needs mutably -- but paying a refcount pair per *instruction* to
        // arrange that is a real cost, and a pointer comparison is not.
        let mut program = Rc::clone(&self.program);
        loop {
            // A namespace a module imported from itself shows bindings this very frame owns, so
            // it can go stale between any two instructions. Catching it up here rather than at
            // each of the dozens of places a namespace can be observed is what makes "no read
            // sees it stale" a property of the loop instead of a claim about a list of sites.
            // Empty for every module that does not import itself, so the ordinary run pays one
            // comparison against a length -- next to the step-limit check below, nothing.
            if !self.self_bindings.is_empty() || !self.self_namespaces.is_empty() {
                self.refresh_self_imports();
            }
            // Cycle collection is pressure-triggered and runs only between instructions, after
            // self-import namespace snapshots catch up with their live cells. The outermost-run
            // guard excludes nested interpreter entries whose Rust callers may retain RefCell
            // borrows. The weak census preserves ordinary VM/Rust roots.
            if outermost_run && self.steps & 0x3ff == 0 {
                let module_map = self.gc_module_map();
                let (module_allocations, module_threshold) = {
                    let module_map = module_map.borrow();
                    (
                        module_map.gc_cycle_candidate_allocations,
                        module_map.gc_cycle_candidate_threshold,
                    )
                };
                if crate::gc_census::automatic_collection_due(module_allocations, module_threshold)
                {
                    self.collect_closure_cycles();
                }
            }
            // One comparison, two boundaries. `checkpoint` is `max_steps` for every program that
            // has no second agent, so this is the step-limit check it always was; when a second
            // agent exists it is also the end of this agent's quantum, and the slow path decides
            // which of the two arrived.
            if self.steps >= self.checkpoint {
                self.reach_checkpoint()?;
            }
            // Ordered so the ordinary run pays one comparison against a constant: `stop_before`
            // is unreachable then, and the frame depth is never looked at.
            if self.instruction >= stop_before && self.frames.len() == stop_depth {
                return Ok(ExecutionResult {
                    value: Value::Undefined,
                    steps: self.steps,
                    process_exit_code: None,
                });
            }
            if !Rc::ptr_eq(&program, &self.program) {
                program = Rc::clone(&self.program);
            }
            let Some(opcode) = Self::code(&program, self.function.as_ref()).get(self.instruction)
            else {
                return Err(ExecutionError::InvalidInstructionPointer {
                    instruction: self.instruction,
                });
            };
            let current = self.instruction;
            self.steps += 1;
            self.instruction += 1;

            match opcode {
                Instruction::Const(index) => {
                    let Some(value) = program.constants.get(*index as usize) else {
                        return Err(ExecutionError::ConstantOutOfBounds {
                            index: *index as usize,
                        });
                    };
                    self.stack.push(value.clone().into());
                }
                Instruction::LoadLocal(index) => {
                    let Some(value) = self.locals.get(*index as usize) else {
                        return Err(ExecutionError::LocalOutOfBounds {
                            index: *index as usize,
                        });
                    };
                    let value = value.clone();
                    if matches!(value, Value::Native(Native::Uninitialized, _)) {
                        self.throw_error(
                            ErrorKind::Reference,
                            "cannot access a binding before its declaration".to_owned(),
                            current,
                        )?;
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::StoreLocal(index) => {
                    let value = self.pop(current)?;
                    let Some(local) = self.locals.get_mut(*index as usize) else {
                        return Err(ExecutionError::LocalOutOfBounds {
                            index: *index as usize,
                        });
                    };
                    *local = value;
                }
                Instruction::Pop => {
                    self.pop(current)?;
                }
                Instruction::Dup => {
                    let value =
                        self.stack
                            .last()
                            .cloned()
                            .ok_or(ExecutionError::StackUnderflow {
                                instruction: current,
                            })?;
                    self.stack.push(value);
                }
                Instruction::Unary(operator) => {
                    let value = self.pop(current)?;
                    if *operator == UnaryOp::ToString {
                        let converted = self.as_text_units(&value, current);
                        if let Some(text) = self.catchable(converted, current)? {
                            self.stack.push(Value::String(text));
                        }
                        continue;
                    }
                    // Numbers cannot invoke coercion or fail a unary operation. Reuse the
                    // scalar implementation without constructing two fallible call results.
                    if matches!(value, Value::Number(_)) {
                        self.stack.push(unary_of_number(*operator, &value));
                        continue;
                    }
                    // Only the numeric operators coerce. `typeof` reports a type, `void`
                    // discards, and `!` asks whether the value is truthy -- an object always is,
                    // whatever its `valueOf` would have said, so reducing it would both be wrong
                    // and run the program's code for no reason.
                    let value = if matches!(
                        operator,
                        UnaryOp::Negate
                            | UnaryOp::Plus
                            | UnaryOp::BitwiseNot
                            | UnaryOp::ToNumeric
                            | UnaryOp::Increment
                            | UnaryOp::Decrement
                    ) {
                        // Through `catchable`, because reducing can fail in a way the program is
                        // allowed to catch: an object with no route to a primitive is a
                        // `TypeError`, not the end of the run.
                        let reduced = self.as_primitive(&value, PrimitiveHint::Number, current);
                        let Some(reduced) = self.catchable(reduced, current)? else {
                            continue;
                        };
                        reduced
                    } else {
                        value
                    };
                    let computed = unary(*operator, value);
                    // Through `catchable` for the same reason the reduction above is: `+1n` and
                    // `-Symbol()` are `TypeError`s the program is entitled to catch.
                    let Some(computed) = self.catchable(computed, current)? else {
                        continue;
                    };
                    self.stack.push(computed);
                }
                // `in` consults a proxy's `has` trap, so it cannot go through the pure operator
                // table the way the arithmetic ones do.
                // A proxy answers `in` with a trap, and an object on the left is a key that
                // `ToPropertyKey` has to reduce first -- `({toString(){return "K"}}) in o` asks
                // about `K`. Either way the operator needs the machine, which `binary` is not.
                // Any object receiver, not only a proxy: a proxy reached through the prototype
                // chain has to run its `has` trap too, and only the machine path can call one.
                // The pure path below still answers for a primitive receiver, which throws.
                Instruction::Binary(BinaryOp::In)
                    if matches!(self.stack.last(), Some(right) if is_reflect_target(right))
                        || matches!(
                            self.stack
                                .len()
                                .checked_sub(2)
                                .and_then(|at| self.stack.get(at)),
                            Some(key) if is_reflect_target(key)
                        ) =>
                {
                    let right = self.pop(current)?;
                    let left = self.pop(current)?;
                    let coerced = self.computed_key(&left, current);
                    let Some(left) = self.catchable(coerced, current)? else {
                        continue;
                    };
                    let found = self.has(&right, &left, current);
                    let Some(found) = self.catchable(found, current)? else {
                        continue;
                    };
                    self.stack.push(Value::Boolean(found));
                }
                Instruction::Binary(BinaryOp::InstanceOf) => {
                    let right = self.pop(current)?;
                    let left = self.pop(current)?;
                    // Threaded rather than global: the error prototypes belong to this run, and
                    // creating the one being tested against is what makes the comparison
                    // meaningful for a kind nothing has constructed yet.
                    let prototype = match &right {
                        Value::Native(Native::Error(kind), _) => Some(self.error_prototype(*kind)),
                        _ => None,
                    };
                    let answer = self.instance_of_value(&left, &right, prototype.as_ref(), current);
                    let Some(answer) = self.catchable(answer, current)? else {
                        continue;
                    };
                    self.stack.push(Value::Boolean(answer));
                }
                Instruction::Binary(operator) => {
                    let right = self.pop(current)?;
                    let left = self.pop(current)?;
                    // Every operator that coerces its operands reduces an object to a primitive
                    // first, which is a *call* -- so it cannot happen inside `binary`, which has
                    // no machine to call with. That is all of the arithmetic and the ordering
                    // comparisons: `a - b` where both are wrappers around a number is how every
                    // date and decimal library is used, and it was reaching `toString` and
                    // producing NaN. The equality operators are excluded because they compare by
                    // identity when both sides are objects, and `in`/`instanceof` because their
                    // right side is a container rather than a value.
                    // Every operator that coerces its operands reduces an object to a primitive
                    // first, which is a *call* -- so it cannot happen inside `binary`, which has
                    // no machine to call with. `a - b` where both sides wrap a number is how
                    // every date and decimal library is used, and it was reaching `toString` and
                    // producing NaN.
                    //
                    // `==` is the awkward one: it compares two objects by identity and reduces
                    // only when the other side is a primitive. `===` never reduces, and
                    // `in`/`instanceof` take a container on the right rather than a value.
                    // `x == null` compares against nullish, which the specification answers
                    // without any conversion -- so reducing there would run the program's
                    // `valueOf` for a question that never needed it. `value != null` is the
                    // commonest guard in JavaScript, and calling into a wrapper's accessor to
                    // evaluate it is both wrong and, for a wrapper that assumes it is being used
                    // properly, fatal.
                    let mixed = reduces_to_primitive(&left) != reduces_to_primitive(&right)
                        && !left.is_nullish()
                        && !right.is_nullish();
                    let reduces =
                        matches!(
                            operator,
                            BinaryOp::Add
                                | BinaryOp::Subtract
                                | BinaryOp::Multiply
                                | BinaryOp::Divide
                                | BinaryOp::Remainder
                                | BinaryOp::Exponentiate
                                | BinaryOp::LessThan
                                | BinaryOp::LessThanOrEqual
                                | BinaryOp::GreaterThan
                                | BinaryOp::GreaterThanOrEqual
                                | BinaryOp::BitwiseAnd
                                | BinaryOp::BitwiseOr
                                | BinaryOp::BitwiseXor
                                | BinaryOp::ShiftLeft
                                | BinaryOp::ShiftRight
                                | BinaryOp::UnsignedShiftRight
                        ) || (matches!(operator, BinaryOp::LooseEqual | BinaryOp::LooseNotEqual)
                            && mixed);
                    // `+` and loose equality ask for no hint at all -- the default -- because
                    // each chooses between a string reading and a numeric one only after both
                    // sides have reduced. Every other operator here has already committed to a
                    // number before it looks at either side. The distinction is invisible for
                    // every value but a `Date`, and it is the whole of why `date + date`
                    // concatenates two date strings while `date - date` subtracts two instants.
                    let hint = if matches!(
                        operator,
                        BinaryOp::Add | BinaryOp::LooseEqual | BinaryOp::LooseNotEqual
                    ) {
                        PrimitiveHint::Default
                    } else {
                        PrimitiveHint::Number
                    };
                    let (left, right) = if reduces {
                        // Left before right, and each through `catchable`: the reduction runs
                        // user code, so it can both fail catchably and observe the order.
                        let reduced = self.as_primitive(&left, hint, current);
                        let Some(left) = self.catchable(reduced, current)? else {
                            continue;
                        };
                        // `ToNumeric(lhs)` completes before the right operand is reduced at
                        // all, for the operators that have no string reading to decide first.
                        // A symbol has no number, so `Symbol() * f()` refuses *here* rather
                        // than after calling `f`'s `valueOf` -- one observable call that the
                        // language says does not happen. `+` and the relational operators are
                        // deliberately absent: both choose between a string and a numeric
                        // reading, which needs each side reduced before either is converted.
                        if numeric_left_to_right(*operator) && matches!(left, Value::Symbol(_)) {
                            self.catchable::<Value>(
                                Err(ExecutionError::SymbolIsNotANumber),
                                current,
                            )?;
                            continue;
                        }
                        let reduced = self.as_primitive(&right, hint, current);
                        let Some(right) = self.catchable(reduced, current)? else {
                            continue;
                        };
                        (left, right)
                    } else {
                        (left, right)
                    };
                    if let Some(outcome) = bigint_binary(*operator, &left, &right) {
                        let Some(value) = self.catchable(outcome, current)? else {
                            continue;
                        };
                        self.stack.push(value);
                        continue;
                    }
                    // Concatenation can refuse a result too long to build, and that refusal is
                    // a `RangeError` a program is meant to catch -- so it goes through the same
                    // conversion the bigint arm above uses rather than out of the loop.
                    let Some(value) =
                        self.catchable(binary(&self.intrinsics, *operator, left, right), current)?
                    else {
                        continue;
                    };
                    self.stack.push(value);
                }
                Instruction::JumpIfFalse(target) => {
                    if !self.pop(current)?.is_truthy() {
                        self.jump(current, *target)?;
                    }
                }
                Instruction::JumpIfTruthy(target) => {
                    if self.pop(current)?.is_truthy() {
                        self.jump(current, *target)?;
                    }
                }
                Instruction::JumpIfNotNullish(target) => {
                    if !self.pop(current)?.is_nullish() {
                        self.jump(current, *target)?;
                    }
                }
                Instruction::JumpIfReturnSignal(target) => {
                    if matches!(
                        self.stack.last(),
                        Some(Value::Native(Native::ReturnSignal, _))
                    ) {
                        self.jump(current, *target)?;
                    }
                }
                Instruction::Jump(target) => self.jump(current, *target)?,
                Instruction::HostCall { op, argc } => {
                    let argc = usize::from(*argc);
                    if self.stack.len() < argc {
                        return Err(ExecutionError::StackUnderflow {
                            instruction: current,
                        });
                    }
                    let args_start = self.stack.len() - argc;
                    let mut args = self.stack.drain(args_start..).collect::<Vec<_>>();
                    let line = matches!(op, HostOp::WriteStdout | HostOp::WriteStderr);
                    if line {
                        args = self.console_arguments(args, current)?;
                    }
                    let args = args.into_iter().map(HostValue::from).collect::<Vec<_>>();
                    let result = if line {
                        self.host_line(*op == HostOp::WriteStderr, args)
                    } else {
                        self.host.call(*op, &args)
                    }
                    .map_err(|error| ExecutionError::Host {
                        instruction: current,
                        message: error.to_string(),
                    })?;
                    self.stack.push(result.into());
                }
                Instruction::MakeFunction(index) => {
                    let closure = self.make_closure(*index)?;
                    self.stack.push(Value::Function(closure));
                }
                Instruction::CallMethod { name, argc } => {
                    let name = self.string_constant_units(*name, current)?;
                    let arguments = self.take_call_arguments(*argc, current)?;
                    let receiver = self.pop(current)?;
                    let read =
                        self.read_property_key(&receiver, PropertyKeyRef::Units(&name), current);
                    let Some(method) = self.catchable(read, current)? else {
                        continue;
                    };
                    // Named, because "undefined is not a function" is the least useful message a
                    // runtime produces and the name is right here.
                    if !is_callable(&method) {
                        let shown = name.to_lossy_string();
                        self.throw_error(
                            ErrorKind::Type,
                            format!("{}.{shown} is not a function", receiver.type_name()),
                            current,
                        )?;
                        continue;
                    }
                    let invoked = self.invoke(method, receiver, arguments, current, Some(name));
                    let Some(result) = self.catchable(invoked, current)? else {
                        continue;
                    };
                    if let Some(result) = result {
                        self.stack.push(result);
                    }
                }
                Instruction::CallMethodPrepared { name, argc } => {
                    let name = self.string_constant_units(*name, current)?;
                    let arguments = self.take_call_arguments(*argc, current)?;
                    let method = self.pop(current)?;
                    let receiver = self.pop(current)?;
                    if !is_callable(&method) {
                        let shown = name.to_lossy_string();
                        self.throw_error(
                            ErrorKind::Type,
                            format!("{}.{shown} is not a function", receiver.type_name()),
                            current,
                        )?;
                        continue;
                    }
                    let invoked = self.invoke(method, receiver, arguments, current, Some(name));
                    let Some(result) = self.catchable(invoked, current)? else {
                        continue;
                    };
                    if let Some(result) = result {
                        self.stack.push(result);
                    }
                }
                Instruction::CallComputed { argc } => {
                    let arguments = self.take_call_arguments(*argc, current)?;
                    let method = self.pop(current)?;
                    let key = self.pop(current)?;
                    let receiver = self.pop(current)?;
                    let method_name = match &key {
                        Value::String(name) => Some(name.clone()),
                        _ => None,
                    };
                    let invoked = self.invoke(method, receiver, arguments, current, method_name);
                    let Some(result) = self.catchable(invoked, current)? else {
                        continue;
                    };
                    if let Some(result) = result {
                        self.stack.push(result);
                    }
                }
                Instruction::Dup2 => {
                    let len = self.stack.len();
                    let (first, second) = (
                        self.stack
                            .get(len.wrapping_sub(2))
                            .ok_or(ExecutionError::StackUnderflow {
                                instruction: current,
                            })?
                            .clone(),
                        self.stack
                            .get(len.wrapping_sub(1))
                            .ok_or(ExecutionError::StackUnderflow {
                                instruction: current,
                            })?
                            .clone(),
                    );
                    self.stack.push(first);
                    self.stack.push(second);
                }
                Instruction::NewArray { count } => {
                    let count = *count as usize;
                    if self.stack.len() < count {
                        return Err(ExecutionError::StackUnderflow {
                            instruction: current,
                        });
                    }
                    let items = self.stack.split_off(self.stack.len() - count);
                    self.stack.push(Value::Array(ArrayData::new(items)));
                }
                Instruction::IterableToArray => {
                    let source = self.pop(current)?;
                    // Everything goes through the iteration protocol, an array included. An
                    // array is already the shape the pattern wants, but destructuring is defined
                    // as `GetIterator` and the difference is observable: after
                    // `delete Array.prototype[Symbol.iterator]`, `var [x] = [1]` is a TypeError,
                    // and a replaced array iterator is what the pattern must bind from. Reading
                    // the elements directly answered from the array itself and could see
                    // neither.
                    //
                    // The walk has to go through `catchable`: `var [a] = null` is a `TypeError`
                    // the program is meant to catch, and letting the error out of the loop
                    // aborted the whole module -- taking every unrelated test in the file with
                    // it, and doing so where no `try` could see it.
                    let drained = self.drain_iterable(&source, current);
                    let Some(items) = self.catchable(drained, current)? else {
                        continue;
                    };
                    self.stack.push(Value::Array(ArrayData::new(items)));
                }
                Instruction::SpreadInto => {
                    let source = self.pop(current)?;
                    let target = self.pop(current)?;
                    let Value::Array(items) = &target else {
                        return Err(ExecutionError::NotAnObject {
                            instruction: current,
                            value: target.type_name(),
                        });
                    };
                    // Spread consumes any iterable through the protocol, an array included:
                    // `[...arr]` is defined as `GetIterator(arr)`, so a removed or replaced
                    // `Array.prototype[Symbol.iterator]` decides what the spread produces --
                    // or that it throws. Copying an array's elements directly answered from the
                    // array and could see neither.
                    let drained = self.drain_iterable(&source, current);
                    let Some(collected) = self.catchable(drained, current)? else {
                        continue;
                    };
                    items.borrow_mut().extend(collected);
                    self.stack.push(target);
                }
                Instruction::ToPropertyKey => {
                    let key = self.pop(current)?;
                    // A nullish object throws before the key converts, so leave the key alone and
                    // let the read below produce the error rather than duplicating it here.
                    if self.stack.last().is_some_and(Value::is_nullish) {
                        self.stack.push(key);
                        continue;
                    }
                    // The conversion is the program's own `toString`, so it can throw, and the
                    // throw belongs to whatever `try` encloses the member expression.
                    let coerced = self.computed_key(&key, current);
                    let Some(key) = self.catchable(coerced, current)? else {
                        continue;
                    };
                    self.stack.push(key);
                }
                Instruction::GetComputed => {
                    let key = self.pop(current)?;
                    let target = self.pop(current)?;
                    if target.is_nullish() {
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "Cannot read properties of {} (reading '{}')",
                                target.to_display_string(),
                                key.to_display_string()
                            ),
                            current,
                        )?;
                        continue;
                    }
                    let read = self
                        .prepare_computed_read(&target, &key, &target, current)
                        .and_then(|read| self.dispatch_property_read(read, current));
                    let Some(value) = self.catchable(read, current)? else {
                        continue;
                    };
                    if let Some(value) = value {
                        self.stack.push(value);
                    }
                }
                Instruction::SetComputed => {
                    let value = self.pop(current)?;
                    let key = self.pop(current)?;
                    let target = self.pop(current)?;
                    let assigned = self.assign(&target, &key, value.clone(), current);
                    if self.catchable(assigned, current)?.is_none() {
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::SetComputedStrict { strict } => {
                    let value = self.pop(current)?;
                    let key = self.pop(current)?;
                    let target = self.pop(current)?;
                    let assigned =
                        self.assign_with_strict(&target, &key, value.clone(), current, *strict);
                    if self.catchable(assigned, current)?.is_none() {
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::DefineOwnProperty(name) => {
                    let value = self.pop(current)?;
                    let target = self.pop(current)?;
                    let name = self.string_constant_units(*name, current)?;
                    let descriptor = Descriptor {
                        value: Some(value.clone()),
                        writable: Some(true),
                        enumerable: Some(true),
                        configurable: Some(true),
                        ..Descriptor::default()
                    };
                    let defined = self.define_own_operation_key(
                        &target,
                        PropertyKeyRef::Units(&name),
                        &descriptor,
                        current,
                    );
                    let Some(accepted) = self.catchable(defined, current)? else {
                        continue;
                    };
                    if !accepted {
                        self.throw_error(
                            ErrorKind::Type,
                            "Cannot define class field".into(),
                            current,
                        )?;
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::DefineOwnComputed => {
                    let value = self.pop(current)?;
                    let key = self.pop(current)?;
                    let target = self.pop(current)?;
                    let Some(key_units) =
                        self.catchable(key.property_key_units_checked(), current)?
                    else {
                        continue;
                    };
                    let descriptor = Descriptor {
                        value: Some(value.clone()),
                        writable: Some(true),
                        enumerable: Some(true),
                        configurable: Some(true),
                        ..Descriptor::default()
                    };
                    let defined = self.define_own_operation_key(
                        &target,
                        PropertyKeyRef::from_value(&key, &key_units),
                        &descriptor,
                        current,
                    );
                    let Some(accepted) = self.catchable(defined, current)? else {
                        continue;
                    };
                    if !accepted {
                        self.throw_error(
                            ErrorKind::Type,
                            "Cannot define class field".into(),
                            current,
                        )?;
                        continue;
                    }
                    self.stack.push(value);
                }
                // The read half of `SetPrototype`, and `super.x`'s first step: the object a
                // method was defined on answers with the link it carries at this moment rather
                // than the one it carried when the class was written.
                Instruction::LoadPrototype => {
                    let value = self.pop(current)?;
                    let prototype = prototype_of(&value);
                    self.stack.push(prototype);
                }
                Instruction::SetPrototype => {
                    let prototype = self.pop(current)?;
                    let target = self.pop(current)?;
                    // `class D extends X` links the two constructors with this instruction, and
                    // a *function* target is what tells that apart from `{ __proto__: x }`,
                    // whose target is always the new object. The specification checks the
                    // heritage at definition: anything that is not `null` and not a constructor
                    // is a `TypeError`. Without the check `class D extends 5 {}` and
                    // `class D extends (() => {}) {}` both built a class whose `super()` could
                    // never work -- the failure arriving at construction, or not at all.
                    if matches!(target, Value::Function(_))
                        && !matches!(prototype, Value::Null)
                        && !is_constructor(&prototype)
                    {
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "class heritage {} is not a constructor",
                                prototype.to_display_string()
                            ),
                            current,
                        )?;
                        continue;
                    }
                    // Only an object or `null` can be a prototype, and `{ __proto__: 1 }` is
                    // neither an error nor a property: the literal quietly keeps the prototype
                    // it already had. Recording the primitive instead would make
                    // `Object.getPrototypeOf` answer `1`, which is a chain no walk can end.
                    if matches!(
                        prototype,
                        Value::Undefined
                            | Value::Number(_)
                            | Value::String(_)
                            | Value::Boolean(_)
                            | Value::Symbol(_)
                            | Value::BigInt(_)
                    ) {
                        self.stack.push(target);
                        continue;
                    }
                    match &target {
                        Value::Object(object) => object.borrow_mut().set_prototype(prototype),
                        Value::Function(closure) => {
                            // A derived constructor inherits its parent's statics through the
                            // constructor's own property store.
                            closure.properties.borrow_mut().set_prototype(prototype);
                        }
                        other => {
                            return Err(ExecutionError::NotAnObject {
                                instruction: current,
                                value: other.type_name(),
                            });
                        }
                    }
                    self.stack.push(target);
                }
                Instruction::SuperForward => {
                    let parent = self.pop(current)?;
                    let arguments = self.arguments.clone();
                    self.stack.push(parent);
                    let argc = arguments.len();
                    for argument in arguments {
                        self.stack.push(argument);
                    }
                    // Fall through to the shared super-call path by re-dispatching with a known
                    // count: forwarding differs from `super(a, b)` only in where the count came
                    // from.
                    let called = self.super_call(argc as u16, current);
                    // Through `catchable` for the same reason `New` is: a parent that is not a
                    // constructor -- `class S extends Symbol` -- is a `TypeError` the program is
                    // entitled to catch, and a bare `?` ended the run instead.
                    if self.catchable(called, current)?.is_none() {
                        continue;
                    }
                }
                Instruction::SuperCall { argc } => {
                    let called = self.super_call(*argc, current);
                    if self.catchable(called, current)?.is_none() {
                        continue;
                    }
                }
                Instruction::SuperApply => {
                    let arguments = self.pop(current)?;
                    let Value::Array(arguments) = arguments else {
                        // The compiler only ever builds this operand with `array_literal`.
                        return Err(ExecutionError::NotAnObject {
                            instruction: current,
                            value: arguments.type_name(),
                        });
                    };
                    let arguments = arguments.borrow().clone();
                    let called = self.super_call_with(arguments, current);
                    if self.catchable(called, current)?.is_none() {
                        continue;
                    }
                }
                Instruction::PushHandler(target) => {
                    self.handlers.push(Handler {
                        target: *target as usize,
                        frame_depth: self.frames.len(),
                        stack_height: self.stack.len(),
                        native_depth: self.native_depth,
                    });
                }
                Instruction::PopHandler => {
                    self.handlers.pop();
                }
                // Identical in the machine: both leave the generator's registers in place for
                // the caller to snapshot, where a `Return` would have popped the frame. They
                // differ only in what they tell the driver, which is recorded on the state by
                // whoever resumed it.
                Instruction::Yield | Instruction::Await => {
                    let value = self.pop(current)?;
                    self.suspended = true;
                    self.suspended_awaiting = matches!(opcode, Instruction::Await);
                    return Ok(ExecutionResult {
                        value,
                        steps: self.steps,
                        process_exit_code: None,
                    });
                }
                Instruction::YieldStar(exit_target) => {
                    let value = self.pop(current)?;
                    let iterator = self.pop(current)?;
                    let wrapped = match self.pop(current)? {
                        Value::Boolean(b) => b,
                        _ => false,
                    };
                    self.suspended = true;
                    self.suspended_awaiting = false;
                    self.suspended_delegate = Some((iterator, *exit_target as usize, wrapped));
                    return Ok(ExecutionResult {
                        value,
                        steps: self.steps,
                        process_exit_code: None,
                    });
                }
                Instruction::ToObject => {
                    let value = self.pop(current)?;
                    if value.is_nullish() {
                        // `with (null)` throws before its body runs, which is why the coercion
                        // is a step of its own rather than something the first name lookup does.
                        self.throw_error(
                            ErrorKind::Type,
                            format!("Cannot convert {} to object", value.to_display_string()),
                            current,
                        )?;
                        continue;
                    }
                    self.stack.push(match &value {
                        Value::String(_)
                        | Value::Number(_)
                        | Value::Boolean(_)
                        | Value::Symbol(_)
                        | Value::BigInt(_) => wrap_primitive(value),
                        _ => value,
                    });
                }
                Instruction::WithHas(name) => {
                    let name = self.string_constant_units(*name, current)?;
                    let object = self.pop(current)?;
                    let present =
                        has_property_key(&self.intrinsics, &object, PropertyKeyRef::Units(&name));
                    if !present {
                        self.stack.push(Value::Boolean(false));
                        continue;
                    }
                    // `Symbol.unscopables` is how an object takes a name *out* of the scope it
                    // provides. It is what let `Array.prototype.flat` be added years after code
                    // like `with (array) { flat }` was written without changing what that code
                    // means, and it is an ordinary property read: a getter on it runs, and what
                    // it throws is the program's to handle.
                    let read =
                        self.read_property(&object, WellKnown::Unscopables.property_key(), current);
                    let Some(blockers) = self.catchable(read, current)? else {
                        continue;
                    };
                    let blocked = if matches!(blockers, Value::Undefined | Value::Null) {
                        false
                    } else {
                        let read = self.read_property_key(
                            &blockers,
                            PropertyKeyRef::Units(&name),
                            current,
                        );
                        let Some(flag) = self.catchable(read, current)? else {
                            continue;
                        };
                        flag.is_truthy()
                    };
                    self.stack.push(Value::Boolean(!blocked));
                }
                instruction @ (Instruction::DynamicImport
                | Instruction::DynamicImportWithOptions) => {
                    let raw_options =
                        if matches!(instruction, Instruction::DynamicImportWithOptions) {
                            Some(self.pop(current)?)
                        } else {
                            None
                        };
                    let specifier = self.pop(current)?;
                    let promise = PromiseState::pending();
                    let text = match self.import_specifier(&specifier, current)? {
                        Ok(text) => text,
                        Err(thrown) => {
                            self.settle(&promise, &Settled::Rejected(thrown));
                            self.stack.push(Value::Promise(promise));
                            continue;
                        }
                    };
                    let _options = match raw_options {
                        Some(options) => {
                            match self.normalize_import_attributes(&options, current)? {
                                Ok(attributes) => attributes,
                                Err(thrown) => {
                                    self.settle(&promise, &Settled::Rejected(thrown));
                                    self.stack.push(Value::Promise(promise));
                                    continue;
                                }
                            }
                        }
                        None => None,
                    };
                    // Everything from the coercion onwards rejects rather than throws: the
                    // specification hands the promise back first and only then looks at what it
                    // was given, so `import({ toString() { throw x } })` is a rejection and not
                    // an exception at the call site.
                    let generation_owner = self
                        .module_map
                        .borrow_mut()
                        .import_generation_handle_for_id(
                            self.current_realm,
                            self.module_path.as_deref(),
                            self.module_generation,
                        );
                    let import_generation = generation_owner.borrow().id;
                    let scheduled = self.scheduled(Job::Import(
                        Rc::clone(&promise),
                        text,
                        _options,
                        self.module_path.clone(),
                        import_generation,
                        generation_owner,
                    ));
                    self.with_realm_mut(|realm| realm.queue_microtask(scheduled));
                    self.stack.push(Value::Promise(promise));
                }
                Instruction::Throw => {
                    let thrown = self.pop(current)?;
                    self.unwind(thrown, current)?;
                }
                Instruction::LoadArguments => {
                    let arguments = self.arguments_object();
                    self.stack.push(arguments);
                }
                // A rest parameter is always the *call's* arguments, arrow or not.
                Instruction::RestArguments(from) => {
                    let rest = self
                        .arguments
                        .get(*from as usize..)
                        .unwrap_or_default()
                        .to_vec();
                    self.stack.push(Value::Array(ArrayData::new(rest)));
                }
                Instruction::LoadThis => {
                    let this_value = self.this_value.clone();
                    self.stack.push(this_value);
                }
                Instruction::LoadNewTarget => {
                    let new_target = self.new_target.clone();
                    self.stack.push(new_target);
                }
                Instruction::GetProperty(name)
                    if matches!(self.stack.last(), Some(Value::Native(Native::Error(_), _)))
                        && self.string_constant_units(*name, current)?.eq("prototype") =>
                {
                    // `Error.prototype` has to be the same object instances inherit from, or
                    // `extends Error` would link to something no error descends from.
                    let Some(Value::Native(Native::Error(kind), _)) = self.stack.pop() else {
                        unreachable!("checked by the guard");
                    };
                    let prototype = self.error_prototype(kind);
                    self.stack.push(prototype);
                }
                Instruction::DirectEval(site) => {
                    // The compiler read the global `eval` binding before evaluating arguments.
                    // It stays under them so an accessor or replacement follows ordinary call
                    // ordering while the intrinsic case still enters the caller's environment.
                    let arguments = match site.argc {
                        Some(argc) => {
                            let argc = usize::from(argc);
                            if self.stack.len() < argc {
                                return Err(ExecutionError::StackUnderflow {
                                    instruction: current,
                                });
                            }
                            self.stack.split_off(self.stack.len() - argc)
                        }
                        None => match self.pop(current)? {
                            Value::Array(arguments) => arguments.borrow().clone(),
                            argument => vec![argument],
                        },
                    };
                    let callee = self.pop(current)?;
                    if !matches!(&callee, Value::Native(Native::Eval, _)) {
                        let called = self.call_value(&callee, Value::Undefined, arguments, current);
                        if let Some(result) = self.catchable(called, current)? {
                            self.stack.push(result);
                        }
                        continue;
                    }
                    // An eval of anything but a string *is* that value, per the specification,
                    // which is what makes `eval(someObject)` harmless rather than a coercion.
                    let source = match arguments.into_iter().next() {
                        Some(Value::String(source)) => source,
                        Some(other) => {
                            self.stack.push(other);
                            continue;
                        }
                        None => {
                            self.stack.push(Value::Undefined);
                            continue;
                        }
                    };
                    if !self.string_code_generation_allowed {
                        self.throw_error(
                            ErrorKind::Eval,
                            "Code generation from strings disallowed for this context".into(),
                            current,
                        )?;
                        continue;
                    }
                    // Entered rather than driven: the interpreter loop runs the eval's frame
                    // next, and its `Return` leaves the completion value where this instruction's
                    // result belongs. Nothing is pushed here.
                    let entered = self.enter_eval_units(&source, Some(site), false);
                    if self.catchable(entered, current)?.is_none() {
                        continue;
                    }
                }
                Instruction::LoadBuiltin(builtin) => {
                    // Read the running code's realm global, so a secondary realm's bare Array
                    // agrees with other.global.Array (ADR 0038).
                    // A source-level builtin name is a mutable global binding. Bootstrap
                    // replacements and user descriptors must be observed just like any other
                    // global property; LoadIntrinsicBuiltin below keeps the engine identity.
                    let global = self.global_object();
                    let key = PropertyKeyRef::Utf8(builtin.name());
                    if !has_property_key(&self.intrinsics, &global, key) {
                        self.throw_error(
                            ErrorKind::Reference,
                            format!("{} is not defined", builtin.name()),
                            current,
                        )?;
                        continue;
                    }
                    let read = self.read_property(&global, builtin.name(), current);
                    let Some(value) = self.catchable(read, current)? else {
                        continue;
                    };
                    self.stack.push(value);
                }
                Instruction::LoadIntrinsicBuiltin(builtin) => {
                    let value = self.builtin_value(*builtin, self.current_realm);
                    self.stack.push(value);
                }
                Instruction::New { argc } => {
                    let argc = *argc as usize;
                    if self.stack.len() < argc + 1 {
                        return Err(ExecutionError::StackUnderflow {
                            instruction: current,
                        });
                    }
                    let arguments = self.stack.split_off(self.stack.len() - argc);
                    let callee = self.pop(current)?;
                    // `new` never asked whether its callee could be constructed, so an arrow, a
                    // generator and an async function were all constructible -- each answering a
                    // fresh object where the language throws. `is_constructor` has always known
                    // the answer for a closure; nothing consulted it here.
                    //
                    // Deliberately closures only. For a built-in, `is_constructor` answers "owns
                    // a prototype", and that under-reports: `URL` and the Node HTTP server are
                    // both constructors it says no to. Checking them here turned three node
                    // corpus cases, two server ones and one regress case into
                    // `URL is not a constructor` -- a wrong answer traded for a worse one. The
                    // built-in table is incomplete and fixing it is its own job; until then this
                    // asks only where the answer is known to be right.
                    if !is_constructor(&callee) {
                        self.throw_error(
                            ErrorKind::Type,
                            // The wording the existing `NotAConstructor` error already uses --
                            // `a number`, not `5`. Two spellings of one failure would be a
                            // difference a program can see for no reason.
                            format!("a {} is not a constructor", callee.type_name()),
                            current,
                        )?;
                        continue;
                    }
                    let constructed = self.construct(callee, arguments, current);
                    if self.catchable(constructed, current)?.is_none() {
                        continue;
                    }
                }
                Instruction::NewObject { count } => {
                    let count = *count as usize;
                    if self.stack.len() < count * 2 {
                        return Err(ExecutionError::StackUnderflow {
                            instruction: current,
                        });
                    }
                    let flat = self.stack.split_off(self.stack.len() - count * 2);
                    let mut object = ObjectData::default();
                    let mut raised = false;
                    for pair in flat.chunks_exact(2) {
                        // Any value can be a computed key: it is converted the same way a
                        // property read converts one, so `{ [Symbol.iterator]: f }` and
                        // `o[Symbol.iterator]` name the same property. A duplicate key keeps
                        // the last value and the first position, which is what `set` does.
                        // An object key is `ToPropertyKey`'d, which runs the program's own
                        // conversion and may throw.
                        let coerced = self.computed_key(&pair[0], current);
                        let Some(key) = self.catchable(coerced, current)? else {
                            raised = true;
                            break;
                        };
                        let Some(key_units) =
                            self.catchable(key.property_key_units_checked(), current)?
                        else {
                            continue;
                        };
                        object.set_key(
                            PropertyKeyRef::from_value(&key, &key_units),
                            pair[1].clone(),
                        );
                    }
                    if raised {
                        continue;
                    }
                    self.stack.push(Value::Object(tracked_object(object)));
                }
                Instruction::SpreadObject => {
                    let source = self.pop(current)?;
                    let target = self.pop(current)?;
                    let Value::Object(object) = &target else {
                        return Err(ExecutionError::NotAnObject {
                            instruction: current,
                            value: target.type_name(),
                        });
                    };
                    // Spreading `null` or `undefined` contributes nothing rather than throwing,
                    // which is the one place object spread is more forgiving than array spread.
                    let object = Rc::clone(object);
                    // CopyDataProperties snapshots own keys, then checks each descriptor and
                    // reads its value in order. The reflected key retains whether it was a
                    // Symbol: a bare StringUnits name loses that distinction, so a spread of
                    // an Event used to turn its four enumerable symbol slots into strings.
                    // This operation path also runs proxy traps and later getters after any
                    // changes an earlier getter made to the source.
                    // Proxy traps and source accessors are user code. Run the whole abstract
                    // operation under the instruction loop's catchable boundary: letting a
                    // getter's `ThrownAcrossNative` escape this arm turns an ordinary JS throw
                    // into a host failure instead of allowing an enclosing `try` to catch it.
                    let copied = (|| {
                        for key in self.own_keys_reflected(&source, current)? {
                            let Some(descriptor) =
                                self.own_descriptor_value(&source, &key, current)?
                            else {
                                continue;
                            };
                            if !self
                                .read_property(&descriptor, "enumerable", current)?
                                .is_truthy()
                            {
                                continue;
                            }
                            let value = self.read_computed(&source, &key, current)?;
                            let units = key.property_key_units_checked()?;
                            object
                                .borrow_mut()
                                .set_key(PropertyKeyRef::from_value(&key, &units), value);
                        }
                        Ok::<(), ExecutionError>(())
                    })();
                    if self.catchable(copied, current)?.is_none() {
                        continue;
                    }
                    self.stack.push(target);
                }
                Instruction::EnumerateKeys => {
                    let target = self.pop(current)?;
                    let keys = self.enumerate_for_in(&target, current)?;
                    self.stack.push(Value::Array(ArrayData::new(keys)));
                }
                Instruction::Apply => {
                    let arguments = self.pop(current)?;
                    let callee = self.pop(current)?;
                    let this_value = self.pop(current)?;
                    let Value::Array(arguments) = arguments else {
                        // The compiler only ever builds this operand with `array_literal`.
                        return Err(ExecutionError::NotAnObject {
                            instruction: current,
                            value: arguments.type_name(),
                        });
                    };
                    let arguments = arguments.borrow().clone();
                    // Use the same frame dispatch as an ordinary call. Compiled callees return
                    // through `Return`; immediate native results and failures still pass through
                    // the instruction's catchable boundary. Synchronously driving a compiled
                    // callee here made recursive spread calls consume the Rust stack.
                    let called = self.invoke(callee, this_value, arguments, current, None);
                    let Some(result) = self.catchable(called, current)? else {
                        continue;
                    };
                    if let Some(result) = result {
                        self.stack.push(result);
                    }
                }
                Instruction::ConstructSpread => {
                    let arguments = self.pop(current)?;
                    let callee = self.pop(current)?;
                    let Value::Array(arguments) = arguments else {
                        return Err(ExecutionError::NotAnObject {
                            instruction: current,
                            value: arguments.type_name(),
                        });
                    };
                    let arguments = arguments.borrow().clone();
                    // `new` with a computed argument list is the same construction the
                    // fixed-arity `New` performs, so it goes through the same path rather than
                    // repeating the allocate-run-override sequence.
                    let constructed = self.construct(callee, arguments, current);
                    if self.catchable(constructed, current)?.is_none() {
                        continue;
                    }
                }
                Instruction::MarkUninitialized(slot) => {
                    let slot = *slot as usize;
                    let marker = Value::Native(Native::Uninitialized, RealmId::PRIMARY);
                    match self.locals.get_mut(slot) {
                        // A cell may already be in the slot, because the binding is captured and
                        // the cell has to exist before anything captures it.
                        Some(Value::Cell(cell)) => *cell.borrow_mut() = marker,
                        Some(local) => *local = marker,
                        None => return Err(ExecutionError::LocalOutOfBounds { index: slot }),
                    }
                }
                Instruction::ThrowReference(name) => {
                    let name = self.string_constant_units(*name, current)?;
                    // A name the compiler could not place may still be a real binding. The
                    // realm's global object is the last place an identifier resolution looks,
                    // and it holds both what a program put there itself with `globalThis.x = 1`
                    // and every intrinsic this realm registers -- including the ones the
                    // compiler's built-in table does not name, which without this were
                    // unreachable by their own name however completely they were implemented.
                    // `get_own` was the read here, and it is data-only by construction -- so a
                    // global installed as an *accessor* was invisible to identifier resolution
                    // and its name answered `ReferenceError`. `Object.defineProperty(globalThis,
                    // "x", { get() {...} })` then `x` is how a polyfill installs a lazy global,
                    // and it is what test262's `PutValue` cases are built on.
                    let globals = self.global_object();
                    let present =
                        has_property_key(&self.intrinsics, &globals, PropertyKeyRef::Units(&name));
                    if present {
                        // A real read, because an accessor is a call: the getter runs, and it may
                        // throw or -- as those cases do -- delete the very property being read.
                        let value = self.read_property_key(
                            &globals,
                            PropertyKeyRef::Units(&name),
                            current,
                        )?;
                        self.stack.push(value);
                        continue;
                    }
                    self.throw_error(
                        ErrorKind::Reference,
                        format!("{} is not defined", name.to_lossy_string()),
                        current,
                    )?;
                    continue;
                }
                Instruction::DeclareGlobalBinding { name, configurable } => {
                    // `CreateGlobalVarBinding`: the property is created only when the global
                    // object does not already have it, so `var x;` over an existing `x` keeps the
                    // value it had rather than resetting it to `undefined`.
                    let name = self.string_constant_units(*name, current)?;
                    if let Value::Object(globals) = self.global_object() {
                        let mut globals = globals.borrow_mut();
                        if globals.get_own_property_units(&name).is_none() {
                            globals.define_value_units(
                                &name,
                                Value::Undefined,
                                Attributes {
                                    writable: true,
                                    enumerable: true,
                                    configurable: *configurable,
                                },
                            );
                        }
                    }
                }
                Instruction::DeleteGlobalBinding(name) => {
                    // The global object environment record's `DeleteBinding`: an ordinary
                    // `[[Delete]]` on the global object. Absent or removed is `true`, which is
                    // why `delete neverDeclared` and `x = 1; delete x` both answer `true` and
                    // the second really does take the property away -- the `ThrowReference`
                    // fallback above then finds nothing, and a later read of `x` throws.
                    // Non-configurable is `false`, which is what `delete NaN` is.
                    let name = self.string_constant_units(*name, current)?;
                    let globals = self.global_object();
                    let key = Value::String(name);
                    let removed = self.delete(&globals, &key, current)?;
                    self.stack.push(Value::Boolean(removed));
                }
                Instruction::LoadGlobalBindingOrUndefined(name) => {
                    // What `typeof` reads for a name no scope bound: the global object's property
                    // if it has one, and `undefined` if it does not -- which is the answer that
                    // makes `typeof maybeMissing === "undefined"` the feature test it is.
                    let name = self.string_constant_units(*name, current)?;
                    // Accessor-aware for the same reason as identifier resolution above, and it
                    // matters more here: `typeof x === "undefined"` is the feature test, so a
                    // lazily-installed global reported as absent sends the program down the
                    // polyfill branch it did not need.
                    let globals = self.global_object();
                    let present =
                        has_property_key(&self.intrinsics, &globals, PropertyKeyRef::Units(&name));
                    let bound = if present {
                        Some(self.read_property_key(
                            &globals,
                            PropertyKeyRef::Units(&name),
                            current,
                        )?)
                    } else {
                        None
                    };
                    self.stack.push(bound.unwrap_or(Value::Undefined));
                }
                Instruction::RequireGlobalBinding(name) => {
                    // `ResolveBinding` in strict code: an assignment target no scope bound is an
                    // unresolvable reference unless the global object has it, and an
                    // unresolvable one is a `ReferenceError` rather than a new global.
                    let name = self.string_constant_units(*name, current)?;
                    let globals = self.global_object();
                    let present =
                        has_property_key(&self.intrinsics, &globals, PropertyKeyRef::Units(&name));
                    if !present {
                        self.throw_error(
                            ErrorKind::Reference,
                            format!("{} is not defined", name.to_lossy_string()),
                            current,
                        )?;
                        continue;
                    }
                }
                Instruction::StoreGlobalBindingStrict(name) => {
                    // `SetMutableBinding` with `S` true on the global object environment record:
                    // an existing property is written, and a name the realm does not have is the
                    // unresolvable reference strict mode refuses -- decided here rather than at
                    // compile time, because a global installed a line earlier is resolvable and
                    // the compiler cannot see it.
                    let name = self.string_constant_units(*name, current)?;
                    let value = self.pop(current)?;
                    let globals = self.global_object();
                    let present =
                        has_property_key(&self.intrinsics, &globals, PropertyKeyRef::Units(&name));
                    if !present {
                        self.throw_error(
                            ErrorKind::Reference,
                            format!("{} is not defined", name.to_lossy_string()),
                            current,
                        )?;
                        continue;
                    }
                    let key = Value::String(name);
                    self.set_or_throw(&globals, &key, value, current)?;
                }
                Instruction::StoreGlobalBinding(name) => {
                    let name = self.string_constant_units(*name, current)?;
                    let value = self.pop(current)?;
                    // The global object environment record's `SetMutableBinding` with `S = false`
                    // is an ordinary `[[Set]]` on the global object: it creates the property when
                    // it is absent, which is what makes the write visible to every later read --
                    // including the `ThrowReference` fallback that resolves the same name.
                    let globals = self.global_object();
                    if let Value::Object(globals) = globals {
                        globals.borrow_mut().set_units(&name, value);
                    }
                }
                Instruction::PushHole => self
                    .stack
                    .push(Value::Native(Native::Hole, RealmId::PRIMARY)),
                Instruction::NewRegExp { pattern, flags } => {
                    // Keep the pattern in its UTF-16 unit representation: regular expressions
                    // are allowed to carry a lone surrogate, and converting it through a scalar
                    // `String` would replace the unit before the regexp compiler sees it.
                    let pattern = self.string_constant_units(*pattern, current)?;
                    // Flags are an IR spelling, not a language value. The parser only emits
                    // scalar text here, so reject malformed constants explicitly rather than
                    // silently replacing a surrogate in an invalid instruction stream.
                    let flags = self.string_constant_scalar(*flags, current)?;
                    match build_regexp(&pattern, &flags) {
                        Some(regexp) => self.stack.push(regexp),
                        // A pattern oxc parsed but this engine rejects. A SyntaxError is what a
                        // program would get from `new RegExp` with the same text.
                        None => {
                            self.throw_error(
                                ErrorKind::Syntax,
                                format!(
                                    "Invalid regular expression: /{}/{flags}",
                                    pattern.to_lossy_string()
                                ),
                                current,
                            )?;
                            continue;
                        }
                    }
                }
                Instruction::HasPrivate { name, .. } => {
                    let name = self.string_constant_scalar(*name, current)?;
                    let brand = self.pop(current)?;
                    let target = self.pop(current)?;
                    if !is_reflect_target(&target) {
                        self.throw_error(
                            ErrorKind::Type,
                            "right-hand side of private `in` must be an object".to_owned(),
                            current,
                        )?;
                        continue;
                    }
                    let answer = private_brand_matches(&target, &name, &brand)
                        && own_private_element(&target, &name).is_some();
                    self.stack.push(Value::Boolean(answer));
                }
                Instruction::GetPrivate(name) => {
                    let name = self.string_constant_scalar(*name, current)?;
                    let brand = self.pop(current)?;
                    let target = self.pop(current)?;
                    if !private_brand_matches(&target, &name, &brand) {
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "Cannot read private member #{} from an object whose class did not declare it",
                                private_display_name(&name)
                            ),
                            current,
                        )?;
                        continue;
                    }
                    // Own-only, and a miss is a TypeError. That own-only rule *is* the brand
                    // check: an object of another class does not carry the name, however much
                    // of the rest of the shape it shares.
                    let found = own_private_element(&target, &name);
                    let Some((property, _)) = found else {
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "Cannot read private member #{} from an object whose class did not declare it",
                                private_display_name(&name)
                            ),
                            current,
                        )?;
                        continue;
                    };
                    // A getter is a *call*, on the object the name was read from. Handing back
                    // the slot instead answered `class C { get #x() { return 7 } }` with the
                    // getter function, which is the shape of a right answer and the content of
                    // a wrong one.
                    let value = match property {
                        Property::Value(value) => value,
                        Property::Accessor {
                            get: Some(getter), ..
                        } => {
                            let called =
                                self.call_value(&getter, target.clone(), Vec::new(), current);
                            let Some(value) = self.catchable(called, current)? else {
                                continue;
                            };
                            value
                        }
                        // Write-only. `undefined` would be a plausible reading and the wrong
                        // one: the language refuses the read rather than inventing an absence.
                        Property::Accessor { get: None, .. } => {
                            self.throw_error(
                                ErrorKind::Type,
                                format!(
                                    "'#{}' was defined without a getter",
                                    private_display_name(&name)
                                ),
                                current,
                            )?;
                            continue;
                        }
                    };
                    self.stack.push(value);
                }
                Instruction::SetPrivate(name) => {
                    let name = self.string_constant_scalar(*name, current)?;
                    let brand = self.pop(current)?;
                    let value = self.pop(current)?;
                    let target = self.pop(current)?;
                    if !private_brand_matches(&target, &name, &brand) {
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "Cannot write private member #{} to an object whose class did not declare it",
                                private_display_name(&name)
                            ),
                            current,
                        )?;
                        continue;
                    }
                    let Some((property, attributes)) = own_private_element(&target, &name) else {
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "Cannot write private member #{} to an object whose class did not declare it",
                                private_display_name(&name)
                            ),
                            current,
                        )?;
                        continue;
                    };
                    match property {
                        Property::Accessor {
                            set: Some(setter), ..
                        } => {
                            let called = self.call_value(
                                &setter,
                                target.clone(),
                                vec![value.clone()],
                                current,
                            );
                            if self.catchable(called, current)?.is_none() {
                                continue;
                            }
                        }
                        Property::Accessor { set: None, .. } => {
                            self.throw_error(
                                ErrorKind::Type,
                                format!(
                                    "'#{}' was defined without a setter",
                                    private_display_name(&name)
                                ),
                                current,
                            )?;
                            continue;
                        }
                        // A private *method* is the one data element that refuses a write, and
                        // the non-writable slot `DefinePrivate` gives it is what tells it apart
                        // from a field. Silently succeeding here loses the assignment and
                        // reports success, which is worse than the refusal it should have been.
                        Property::Value(_) if !attributes.writable => {
                            self.throw_error(
                                ErrorKind::Type,
                                format!(
                                    "Private method '#{}' is not writable",
                                    private_display_name(&name)
                                ),
                                current,
                            )?;
                            continue;
                        }
                        Property::Value(_) => {
                            let Some(store) = private_slot(&target) else {
                                return Err(ExecutionError::NotAnObject {
                                    instruction: current,
                                    value: target.type_name(),
                                });
                            };
                            store.borrow_mut().set(&name, value.clone());
                        }
                    }
                    self.stack.push(value);
                }
                Instruction::DefinePrivate { name, kind } => {
                    let name = self.string_constant_scalar(*name, current)?;
                    let function = self.pop(current)?;
                    let target = self.pop(current)?;
                    if self
                        .catchable(
                            define_function_name(
                                &function,
                                &Value::String(name.clone().into()),
                                match kind {
                                    PrivateKind::Method => None,
                                    PrivateKind::Getter => Some("get"),
                                    PrivateKind::Setter => Some("set"),
                                },
                            ),
                            current,
                        )?
                        .is_none()
                    {
                        continue;
                    }
                    // Not writable, which is how a read of the slot later tells a method from a
                    // field; configurable, so the second half of a `get`/`set` pair may merge
                    // with the first rather than being refused by `redefinable`.
                    let attributes = Attributes {
                        writable: false,
                        enumerable: false,
                        configurable: true,
                    };
                    let install = |data: &mut ObjectData| match kind {
                        PrivateKind::Method => {
                            data.define_value(&name, function.clone(), attributes);
                        }
                        PrivateKind::Getter => {
                            data.define_accessor(&name, Some(function.clone()), None, attributes);
                        }
                        PrivateKind::Setter => {
                            data.define_accessor(&name, None, Some(function.clone()), attributes);
                        }
                    };
                    let Some(store) = private_slot(&target) else {
                        return Err(ExecutionError::NotAnObject {
                            instruction: current,
                            value: target.type_name(),
                        });
                    };
                    install(&mut store.borrow_mut());
                    self.stack.push(target);
                }
                Instruction::DefineAccessor { setter, enumerable } => {
                    let function = self.pop(current)?;
                    let key = self.pop(current)?;
                    let target = self.pop(current)?;
                    // `{ get [k]() {} }` names the property `ToPropertyKey(k)`, which for an
                    // object is the program's own conversion.
                    let coerced = self.computed_key(&key, current);
                    let Some(key) = self.catchable(coerced, current)? else {
                        continue;
                    };
                    let Some(name) = self.catchable(key.property_key_units_checked(), current)?
                    else {
                        continue;
                    };
                    let property_key = PropertyKeyRef::from_value(&key, &name);
                    if self
                        .catchable(
                            define_function_name(
                                &function,
                                &key,
                                Some(if *setter { "set" } else { "get" }),
                            ),
                            current,
                        )?
                        .is_none()
                    {
                        continue;
                    }
                    let (getter, setter) = if *setter {
                        (None, Some(function))
                    } else {
                        (Some(function), None)
                    };
                    let attributes = Attributes {
                        enumerable: *enumerable,
                        ..Attributes::BUILT_IN
                    };
                    match &target {
                        Value::Object(object) => {
                            object.borrow_mut().define_accessor_key(
                                property_key,
                                getter,
                                setter,
                                attributes,
                            );
                        }
                        // A static accessor lands on the constructor, whose properties live in
                        // its own store rather than in an object.
                        Value::Function(closure) => {
                            closure.properties.borrow_mut().define_accessor_key(
                                property_key,
                                getter,
                                setter,
                                attributes,
                            );
                        }
                        other => {
                            return Err(ExecutionError::NotAnObject {
                                instruction: current,
                                value: other.type_name(),
                            });
                        }
                    }
                    self.stack.push(target);
                }
                // A class's methods are properties of the class, not of anything it makes, so
                // they are not enumerated -- see `Instruction::DefineMethod`.
                // Like `DefineMethod`, but the property is enumerated: an object literal's
                // members are, and a class's methods are not.
                Instruction::DefineNamedProperty { name_function } => {
                    let function = self.pop(current)?;
                    let key = self.pop(current)?;
                    let coerced = self.computed_key(&key, current);
                    let Some(key) = self.catchable(coerced, current)? else {
                        continue;
                    };
                    let Some(name) = self.catchable(key.property_key_units_checked(), current)?
                    else {
                        continue;
                    };
                    let property_key = PropertyKeyRef::from_value(&key, &name);
                    if *name_function
                        && self
                            .catchable(define_function_name(&function, &key, None), current)?
                            .is_none()
                    {
                        continue;
                    }
                    let target = self.stack.last().cloned().unwrap_or(Value::Undefined);
                    if let Value::Object(object) = &target {
                        object.borrow_mut().define_value_key(
                            property_key,
                            function,
                            Attributes::DEFAULT,
                        );
                    }
                }
                Instruction::DefineMethod => {
                    let function = self.pop(current)?;
                    let key = self.pop(current)?;
                    let target = self.pop(current)?;
                    let coerced = self.computed_key(&key, current);
                    let Some(key) = self.catchable(coerced, current)? else {
                        continue;
                    };
                    let Some(name) = self.catchable(key.property_key_units_checked(), current)?
                    else {
                        continue;
                    };
                    let property_key = PropertyKeyRef::from_value(&key, &name);
                    if self
                        .catchable(define_function_name(&function, &key, None), current)?
                        .is_none()
                    {
                        continue;
                    }
                    match &target {
                        Value::Object(object) => {
                            object.borrow_mut().define_value_key(
                                property_key,
                                function,
                                Attributes::BUILT_IN,
                            );
                        }
                        // A static method lands on the constructor, whose properties live in its
                        // own store rather than in an object.
                        Value::Function(closure) => {
                            closure.properties.borrow_mut().define_value_key(
                                property_key,
                                function,
                                Attributes::BUILT_IN,
                            );
                        }
                        other => {
                            return Err(ExecutionError::NotAnObject {
                                instruction: current,
                                value: other.type_name(),
                            });
                        }
                    }
                    self.stack.push(target);
                }
                Instruction::DeleteComputed => {
                    let key = self.pop(current)?;
                    let target = self.pop(current)?;
                    let removed = self.remove(&target, &key, current);
                    let Some(removed) = self.catchable(removed, current)? else {
                        continue;
                    };
                    self.stack.push(Value::Boolean(removed));
                }
                Instruction::DeleteComputedStrict { strict } => {
                    let key = self.pop(current)?;
                    let target = self.pop(current)?;
                    let removed = self.remove_with_strict(&target, &key, current, *strict);
                    let Some(removed) = self.catchable(removed, current)? else {
                        continue;
                    };
                    self.stack.push(Value::Boolean(removed));
                }
                Instruction::DeleteProperty(name) => {
                    let name = self.string_constant_units(*name, current)?;
                    let target = self.pop(current)?;
                    let removed = self.remove(&target, &Value::String(name), current);
                    let Some(removed) = self.catchable(removed, current)? else {
                        continue;
                    };
                    self.stack.push(Value::Boolean(removed));
                }
                Instruction::DeletePropertyStrict { name, strict } => {
                    let name = self.string_constant_units(*name, current)?;
                    let target = self.pop(current)?;
                    let key = Value::String(name);
                    let removed = self.remove_with_strict(&target, &key, current, *strict);
                    let Some(removed) = self.catchable(removed, current)? else {
                        continue;
                    };
                    self.stack.push(Value::Boolean(removed));
                }
                Instruction::GetProperty(name) => {
                    let name = self.string_constant_units(*name, current)?;
                    let target = self.pop(current)?;
                    if target.is_nullish() {
                        let shown = name.to_lossy_string();
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "Cannot read properties of {} (reading '{shown}')",
                                target.to_display_string()
                            ),
                            current,
                        )?;
                        continue;
                    }
                    // Catchable: a proxy trap and a getter are both ordinary code, and what
                    // they throw is the program's to handle.
                    let read = self
                        .prepare_property_read_key(&target, PropertyKeyRef::Units(&name), current)
                        .and_then(|read| self.dispatch_property_read(read, current));
                    let Some(value) = self.catchable(read, current)? else {
                        continue;
                    };
                    if let Some(value) = value {
                        self.stack.push(value);
                    }
                }
                // `super.x`. The prototype answers *which* property, and `this` answers who a
                // getter up there is asked about -- which is the whole reason this is not the
                // ordinary read: a method reaching an inherited accessor through `super` is
                // asking it about the instance, not about the prototype it was found on.
                Instruction::LoadSuperProperty(name) => {
                    let name = self.string_constant_units(*name, current)?;
                    let target = self.pop(current)?;
                    if target.is_nullish() {
                        let shown = name.to_lossy_string();
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "Cannot read properties of {} (reading '{shown}')",
                                target.to_display_string()
                            ),
                            current,
                        )?;
                        continue;
                    }
                    let receiver = self.this_value.clone();
                    let read = self
                        .prepare_computed_read(&target, &Value::String(name), &receiver, current)
                        .and_then(|read| self.dispatch_property_read(read, current));
                    let Some(value) = self.catchable(read, current)? else {
                        continue;
                    };
                    if let Some(value) = value {
                        self.stack.push(value);
                    }
                }
                Instruction::LoadSuperElement => {
                    let target = self.pop(current)?;
                    let key = self.pop(current)?;
                    if target.is_nullish() {
                        let shown = key.to_display_string();
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "Cannot read properties of {} (reading '{shown}')",
                                target.to_display_string()
                            ),
                            current,
                        )?;
                        continue;
                    }
                    let receiver = self.this_value.clone();
                    let read = self
                        .prepare_computed_read(&target, &key, &receiver, current)
                        .and_then(|read| self.dispatch_property_read(read, current));
                    let Some(value) = self.catchable(read, current)? else {
                        continue;
                    };
                    if let Some(value) = value {
                        self.stack.push(value);
                    }
                }
                Instruction::SetSuperPropertyStrict { name, strict } => {
                    let name = self.string_constant_units(*name, current)?;
                    let value = self.pop(current)?;
                    let target = self.pop(current)?;
                    if target.is_nullish() {
                        let shown = name.to_lossy_string();
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "Cannot set properties of {} (setting '{shown}')",
                                target.to_display_string()
                            ),
                            current,
                        )?;
                        continue;
                    }
                    let receiver = self.this_value.clone();
                    let key = Value::String(name);
                    let write = self.set_reflect(&target, &key, value.clone(), &receiver, current);
                    let Some(accepted) = self.catchable(write, current)? else {
                        continue;
                    };
                    if !accepted && *strict {
                        let shown = key.to_display_string();
                        self.throw_error(
                            ErrorKind::Type,
                            format!("Cannot set property '{shown}'"),
                            current,
                        )?;
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::SetSuperElementStrict { strict } => {
                    let value = self.pop(current)?;
                    let target = self.pop(current)?;
                    let key = self.pop(current)?;
                    if target.is_nullish() {
                        let shown = key.to_display_string();
                        self.throw_error(
                            ErrorKind::Type,
                            format!(
                                "Cannot set properties of {} (setting '{shown}')",
                                target.to_display_string()
                            ),
                            current,
                        )?;
                        continue;
                    }
                    let receiver = self.this_value.clone();
                    let write = self.set_reflect(&target, &key, value.clone(), &receiver, current);
                    let Some(accepted) = self.catchable(write, current)? else {
                        continue;
                    };
                    if !accepted && *strict {
                        let shown = key.to_display_string();
                        self.throw_error(
                            ErrorKind::Type,
                            format!("Cannot set property '{shown}'"),
                            current,
                        )?;
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::SetProperty(name) => {
                    let name = self.string_constant_units(*name, current)?;
                    let value = self.pop(current)?;
                    let target = self.pop(current)?;
                    if let Some(private) = name
                        .to_scalar_cow()
                        .ok()
                        .filter(|key| key.starts_with(PRIVATE_KEY_PREFIX))
                    {
                        let Some(store) = private_slot(&target) else {
                            return Err(ExecutionError::NotAnObject {
                                instruction: current,
                                value: target.type_name(),
                            });
                        };
                        store.borrow_mut().set(&private, value.clone());
                        self.stack.push(value);
                        continue;
                    }
                    let assigned =
                        self.assign(&target, &Value::String(name), value.clone(), current);
                    if self.catchable(assigned, current)?.is_none() {
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::SetPropertyStrict { name, strict } => {
                    let name = self.string_constant_units(*name, current)?;
                    let value = self.pop(current)?;
                    let target = self.pop(current)?;
                    if let Some(private) = name
                        .to_scalar_cow()
                        .ok()
                        .filter(|key| key.starts_with(PRIVATE_KEY_PREFIX))
                    {
                        let Some(store) = private_slot(&target) else {
                            return Err(ExecutionError::NotAnObject {
                                instruction: current,
                                value: target.type_name(),
                            });
                        };
                        store.borrow_mut().set(&private, value.clone());
                        self.stack.push(value);
                        continue;
                    }
                    let assigned = self.assign_with_strict(
                        &target,
                        &Value::String(name),
                        value.clone(),
                        current,
                        *strict,
                    );
                    if self.catchable(assigned, current)?.is_none() {
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::NewCell(slot) => {
                    let slot = *slot as usize;
                    let cell = Rc::new(RefCell::new(Value::Undefined));
                    *self
                        .locals
                        .get_mut(slot)
                        .ok_or(ExecutionError::LocalOutOfBounds { index: slot })? =
                        Value::Cell(cell);
                }
                Instruction::LoadCell(slot) => {
                    let cell = self.cell(*slot as usize)?;
                    // Imported bindings can cross a cycle before their provider has
                    // published. Publishing then aliases the canonical export cell through
                    // the pending link cell, so read the complete chain here just as namespace
                    // construction and star expansion do.
                    let value = read_through_cell(&Value::Cell(cell));
                    if matches!(value, Value::Native(Native::Uninitialized, _)) {
                        self.throw_error(
                            ErrorKind::Reference,
                            "cannot access a binding before its declaration".to_owned(),
                            current,
                        )?;
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::StoreCell(slot) => {
                    let value = self.pop(current)?;
                    let cell = self.cell(*slot as usize)?;
                    self.write_cell(&cell, value, current, true)?;
                }
                Instruction::MarkCellFunctionName(slot) => {
                    let cell = self.cell(*slot as usize)?;
                    self.readonly_cells.borrow_mut().mark(
                        &cell,
                        ReadonlyCellKind::FunctionName,
                        None,
                    );
                }
                Instruction::LoadUpvalue(index) => {
                    let value = read_through_cell(&Value::Cell(Rc::clone(&self.upvalue(*index)?)));
                    if matches!(value, Value::Native(Native::Uninitialized, _)) {
                        let message = self.upvalue_name(*index).map_or_else(
                            || "cannot access a binding before its declaration".to_owned(),
                            |name| format!("cannot access binding `{name}` before its declaration"),
                        );
                        self.throw_error(ErrorKind::Reference, message, current)?;
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::StoreUpvalue(index) => {
                    let value = self.pop(current)?;
                    let cell = self.upvalue(*index)?;
                    self.write_cell(&cell, value, current, false)?;
                }
                Instruction::Call { argc } => {
                    let argc = *argc;
                    let depth = argc as usize;
                    let callee = self
                        .stack
                        .len()
                        .checked_sub(depth + 1)
                        .and_then(|index| self.stack.get(index))
                        .ok_or(ExecutionError::StackUnderflow {
                            instruction: current,
                        })?;
                    match callee {
                        Value::Function(closure) => {
                            let closure = Rc::clone(closure);
                            // Through `catchable`, so running out of frames is a `RangeError`
                            // the program can catch rather than the end of the run. A parser
                            // that reports a nesting limit instead of dying is written exactly
                            // this way, and so is every "is this too deep" guard.
                            let called = self.call(closure, argc, current);
                            if self.catchable(called, current)?.is_none() {
                                continue;
                            }
                        }
                        // A promise executor's `resolve`/`reject`: an ordinary callable value,
                        // which reaches the program as one and so has to be callable here.
                        Value::Settler(..) | Value::Capability(..) | Value::Element(_) => {
                            let callee = callee.clone();
                            let arguments = self.take_arguments(argc, current)?;
                            let called =
                                self.call_value(&callee, Value::Undefined, arguments, current);
                            let Some(result) = self.catchable(called, current)? else {
                                continue;
                            };
                            self.stack.push(result);
                        }
                        // A bound target can be a compiled function. Normalize its receiver
                        // and arguments through the frame dispatcher rather than recursively
                        // driving another interpreter loop for every bound call.
                        Value::Bound(_) => {
                            let callee = callee.clone();
                            let arguments = self.take_arguments(argc, current)?;
                            let called = self.invoke(
                                callee,
                                Value::Undefined,
                                arguments,
                                current,
                                None,
                            );
                            let Some(result) = self.catchable(called, current)? else {
                                continue;
                            };
                            if let Some(result) = result {
                                self.stack.push(result);
                            }
                        }
                        // A proxy has to consult its `apply` trap before anything else can happen.
                        Value::Proxy(_)
                        // A wrapped function defers for the strongest reason of the four: the
                        // call has to cross a realm boundary in both directions, and
                        // `call_value` is where entering the callee's realm happens.
                        | Value::Wrapped(_)
                        | Value::CommonJsRequire(_)
                        | Value::CommonJsResolve(_) => {
                            let callee = callee.clone();
                            let arguments = self.take_arguments(argc, current)?;
                            let called =
                                self.call_value(&callee, Value::Undefined, arguments, current);
                            let Some(result) = self.catchable(called, current)? else {
                                continue;
                            };
                            self.stack.push(result);
                        }
                        Value::Native(native, realm) => {
                            // These realm markers back the non-callable objects that sit in the
                            // function-kind prototype chain. Ordinary `Instruction::Call`
                            // dispatches natives directly for speed, so it must enforce the same
                            // [[Call]] check as `call_value`/`invoke_dispatch` before treating a
                            // marker as a native function. Otherwise `AsyncFunction.prototype()`
                            // silently returned undefined in the interpreter.
                            if matches!(
                                native,
                                Native::AsyncFunctionPrototype
                                    | Native::GeneratorFunctionPrototype
                                    | Native::AsyncGeneratorFunctionPrototype
                            ) {
                                self.throw_error(
                                    ErrorKind::Type,
                                    "a object is not a function".to_owned(),
                                    current,
                                )?;
                                continue;
                            }
                            let native = *native;
                            let realm = *realm;
                            let mut arguments = self.take_arguments(argc, current)?;
                            // `parseInt`'s radix converts with `ToInt32`, and that conversion is
                            // the program's: a radix object's `valueOf` runs. The machine-free
                            // implementation reads it with `as_number`, which cannot call one, so
                            // `parseInt("ff", { valueOf: () => 16 })` answered NaN. A bare global
                            // call arrives here rather than through `call_value`, which is why
                            // the conversion has to be done at this dispatch too.
                            if matches!(native, Native::ParseInt)
                                && arguments.get(1).is_some_and(|radix| {
                                    is_an_object(radix) && !is_callable(radix)
                                })
                            {
                                let radix =
                                    self.as_number_argument(arguments.get(1), current)?;
                                arguments[1] = Value::Number(radix);
                            }
                            let called = self.entering(realm, |vm| {
                                vm.dispatch_native(native, &Value::Undefined, &arguments, current)
                            });
                            let Some(result) = self.catchable(called, current)? else {
                                continue;
                            };
                            self.stack.push(result);
                        }
                        other => {
                            let value = other.type_name();
                            self.throw_error(
                                ErrorKind::Type,
                                format!("a {value} is not a function"),
                                current,
                            )?;
                            continue;
                        }
                    }
                }
                Instruction::LoadGlobal(slot) => {
                    let value = self.global(*slot as usize)?;
                    // A module binding read from inside a function that ran too early: the one
                    // dead-zone read the compiler cannot see, because whether the function is
                    // called before the declaration is a question about the run, not the text.
                    if matches!(value, Value::Native(Native::Uninitialized, _)) {
                        self.throw_error(
                            ErrorKind::Reference,
                            "cannot access a binding before its declaration".to_owned(),
                            current,
                        )?;
                        continue;
                    }
                    self.stack.push(value);
                }
                Instruction::StoreGlobal(slot) => {
                    let value = self.pop(current)?;
                    self.set_global(*slot as usize, value)?;
                }
                Instruction::Nop => {}
                Instruction::Return => {
                    let mut value = self.stack.pop().unwrap_or(Value::Undefined);
                    let Some(frame) = self.frames.pop() else {
                        return Ok(ExecutionResult {
                            value,
                            steps: self.steps,
                            process_exit_code: None,
                        });
                    };
                    // Discard anything the callee left behind. A well-formed program leaves
                    // nothing, but truncating means a malformed one cannot corrupt its caller.
                    self.stack.truncate(frame.stack_base);
                    // A handler belongs to the frame that installed it. `return` out of a `try`
                    // leaves one behind, and a later unrelated throw would then land on a target
                    // in a function that has already returned -- executing whatever instruction
                    // happens to live at that offset in the *current* function.
                    while self
                        .handlers
                        .last()
                        .is_some_and(|handler| handler.frame_depth > self.frames.len())
                    {
                        self.handlers.pop();
                    }
                    self.locals = frame.locals;
                    self.program = frame.program;
                    self.strictness = frame.strictness;
                    self.current_realm = frame.realm;
                    self.strict = frame.strict;
                    self.module_path = frame.module_path;
                    self.module_generation = frame.module_generation;
                    self.module_generation_owner = frame.module_generation_owner;
                    self.function = frame.function;
                    self.method_name = frame.method_name;
                    self.this_value = frame.this_value;
                    self.new_target = frame.new_target;
                    self.arguments = frame.arguments;
                    self.arguments_object = frame.arguments_object;
                    self.mapped_arguments = frame.mapped_arguments;
                    self.global_base = frame.global_base;
                    self.instruction = frame.instruction;
                    if let Some(instance) = frame.constructing
                        && !is_reflect_target(&value)
                    {
                        // A constructor yields its instance unless the body returned an object,
                        // which wins. That override is what makes a factory constructor work,
                        // and it is why an accidental `return 1` is harmless rather than
                        // producing a number where an instance was expected.
                        //
                        // *Any* object wins, not only a plain one: a factory that returns a
                        // proxy, a map or a promise is the whole reason the override exists, and
                        // listing two shapes here made every one of those silently discarded.
                        value = instance;
                    }
                    if frame.super_call {
                        if is_reflect_target(&value) {
                            self.this_value = value.clone();
                            if let Some(caller) = self.frames.last_mut() {
                                caller.constructing = Some(value.clone());
                            }
                        } else {
                            value = self.this_value.clone();
                        }
                    }
                    self.stack.push(value);
                    if self.frames.len() == stop_at {
                        return Ok(ExecutionResult {
                            value: self.stack.pop().unwrap_or(Value::Undefined),
                            steps: self.steps,
                            process_exit_code: None,
                        });
                    }
                }
                Instruction::LoadTemplateObject(site) => {
                    let site_index = *site as usize;
                    if let Some(cached) = self.get_template_object(*site) {
                        self.stack.push(cached);
                    } else {
                        let template_site = match self.program.template_sites.get(site_index) {
                            Some(site) => site.clone(),
                            None => {
                                return Err(ExecutionError::ConstantOutOfBounds {
                                    index: site_index,
                                });
                            }
                        };
                        let raw_items: Vec<Value> = template_site
                            .raw
                            .iter()
                            .map(|s| Value::String(s.clone()))
                            .collect();
                        let raw_array = ArrayData::new(raw_items);
                        raw_array.length_writable.set(false);
                        raw_array.properties.borrow_mut().freeze();

                        let cooked_items: Vec<Value> = template_site
                            .cooked
                            .iter()
                            .map(|s| match s {
                                Some(text) => Value::String(text.clone()),
                                None => Value::Undefined,
                            })
                            .collect();
                        let cooked_array = ArrayData::new(cooked_items);
                        cooked_array.properties.borrow_mut().define_value(
                            "raw",
                            Value::Array(raw_array),
                            Attributes::NONE,
                        );
                        cooked_array.length_writable.set(false);
                        cooked_array.properties.borrow_mut().freeze();

                        let template_obj = Value::Array(cooked_array);
                        self.insert_template_object(*site, template_obj.clone());
                        self.stack.push(template_obj);
                    }
                }
            }
        }
    }

    /// Transfer control to the innermost handler, or fail if there is none.
    ///
    /// Unwinding restores frames one at a time rather than jumping directly, so each caller's
    /// locals, `this` and arguments are put back exactly as `Return` would have -- an exception
    /// crossing a call must leave the machine in the same shape a normal return does.
    pub(crate) fn unwind(
        &mut self,
        thrown: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        // A handler installed below the current Rust re-entry cannot be jumped to from here:
        // the built-in driving this callback is still on the Rust stack, and `run_frames` would
        // carry on executing whatever the jump landed on -- which is the *caller's* code, at the
        // caller's instruction pointer, inside a driver that thinks it is still running a
        // callback. The value travels as an error instead, and the same handler takes it once
        // every Rust frame between here and the instruction loop has returned.
        //
        // A *host* handler is the exception, and the reason the check is on the target rather
        // than on the depth alone: it means the Rust frame immediately outside asked for the
        // value, so returning to it is not crossing a boundary but arriving at one.
        if self.handlers.last().is_some_and(|handler| {
            handler.target != HOST_HANDLER && handler.native_depth < self.native_depth
        }) {
            self.caught = Some(thrown);
            return Err(ExecutionError::ThrownAcrossNative);
        }
        let Some(handler) = self.handlers.pop() else {
            // A `.return()` signal that reaches the top has run every `finally` between the
            // suspension point and here, which is the whole reason it was thrown.
            if matches!(thrown, Value::Native(Native::ReturnSignal, _)) {
                return Err(ExecutionError::GeneratorReturned);
            }
            let trace = captured_trace(&thrown);
            self.uncaught_value = Some(thrown.clone());
            return Err(ExecutionError::Uncaught {
                instruction,
                message: self.describe_uncaught(&thrown, instruction),
                error_constructor: self.error_constructor(&thrown),
                trace,
            });
        };
        if handler.target == HOST_HANDLER {
            // The host asked for the value rather than a jump. Frames and stack are still
            // restored, so the machine is left exactly as an ordinary catch would leave it.
            while self.frames.len() > handler.frame_depth {
                let frame = self.frames.pop().expect("checked by the loop condition");
                self.locals = frame.locals;
                self.program = frame.program;
                self.strictness = frame.strictness;
                self.current_realm = frame.realm;
                self.strict = frame.strict;
                self.module_path = frame.module_path;
                self.module_generation = frame.module_generation;
                self.module_generation_owner = frame.module_generation_owner;
                self.function = frame.function;
                self.method_name = frame.method_name;
                self.this_value = frame.this_value;
                self.new_target = frame.new_target;
                self.arguments = frame.arguments;
                self.arguments_object = frame.arguments_object;
                self.mapped_arguments = frame.mapped_arguments;
                self.global_base = frame.global_base;
                self.instruction = frame.instruction;
            }
            self.stack.truncate(handler.stack_height);
            self.caught = Some(thrown);
            return Err(ExecutionError::CaughtByHost);
        }
        while self.frames.len() > handler.frame_depth {
            let frame = self.frames.pop().expect("checked by the loop condition");
            self.locals = frame.locals;
            self.program = frame.program;
            self.strictness = frame.strictness;
            self.current_realm = frame.realm;
            self.strict = frame.strict;
            self.module_path = frame.module_path;
            self.module_generation = frame.module_generation;
            self.module_generation_owner = frame.module_generation_owner;
            self.function = frame.function;
            self.method_name = frame.method_name;
            self.this_value = frame.this_value;
            self.new_target = frame.new_target;
            self.arguments = frame.arguments;
            self.arguments_object = frame.arguments_object;
            self.mapped_arguments = frame.mapped_arguments;
            self.global_base = frame.global_base;
            self.instruction = frame.instruction;
        }
        self.stack.truncate(handler.stack_height);
        self.stack.push(thrown);
        self.instruction = handler.target;
        Ok(())
    }

    /// Build the object a generator function call produces.
    pub(crate) fn build_generator(
        &mut self,
        closure: &Rc<Closure>,
        argc: u16,
        instruction: usize,
        this_value: Value,
        new_target: Value,
    ) -> Result<Value, ExecutionError> {
        let proto = &closure.program.functions[closure.function as usize];
        let argc = argc as usize;
        if self.stack.len() < argc + 1 {
            return Err(ExecutionError::StackUnderflow { instruction });
        }
        let received = self.stack.split_off(self.stack.len() - argc);
        self.stack.pop();

        let mut locals = vec![Value::Undefined; proto.local_count as usize];
        // The generator retains every actual for `arguments`, rest parameters and resumptions.
        // Locals need only a separate copy of the formal prefix; cloning the whole argument
        // vector here duplicated surplus actuals and allocated a second vector per generator.
        for (slot, argument) in received
            .iter()
            .take(proto.parameter_count as usize)
            .enumerate()
        {
            locals[slot] = argument.clone();
        }

        // Binding the parameters is part of the *call*, not of the first resumption: the
        // language runs FunctionDeclarationInstantiation and only then hands back the generator
        // object, so `function* g([{x}]) {}` called as `g([null])` throws where it is written,
        // and a parameter default's side effect happens whether or not anyone ever steps the
        // generator. Compiling that binding into the body's first instructions -- which is what
        // makes an ordinary call cheap -- deferred all of it to `next()`.
        //
        // An async function is the one shape that must not do this here. Its instantiation
        // failure *rejects* rather than throwing, and running the prologue as the first thing
        // its driver does already gives exactly that.
        let start = if proto.prologue_end > 0 && !(proto.asynchronous && !proto.async_generator) {
            let prologue_end = proto.prologue_end as usize;
            self.run_prologue(
                closure,
                prologue_end,
                &mut locals,
                &this_value,
                &new_target,
                &received,
            )?;
            prologue_end
        } else {
            0
        };

        let generator = Rc::new(GeneratorData {
            properties: RefCell::new(ObjectData::default()),
            state: RefCell::new(GeneratorState {
                closure: Rc::clone(closure),
                locals,
                stack: Vec::new(),
                handlers: Vec::new(),
                instruction: start,
                // Captured at the call, not at the first resumption: the receiver a generator body
                // sees is the one the *call* named, and by the time `next` runs the interpreter is
                // in whatever frame the consumer happens to be in. Dropping it here made `this`
                // undefined in every generator, async function and async method body.
                this_value,
                new_target,
                arguments: received,
                started: false,
                running: false,
                done: false,
                awaiting: false,
                returning: None,
                delegate: None,
            }),
        });
        let module_map = self.gc_module_map();
        let mut module_map = module_map.borrow_mut();
        if module_map.gc_generators.len() >= module_map.gc_generators_prune_at {
            module_map
                .gc_generators
                .retain(|entry| entry.strong_count() > 0);
            module_map.gc_generators_prune_at = module_map
                .gc_generators
                .len()
                .saturating_add(module_map.gc_generators.len().max(1024));
        }
        module_map.gc_generators.push(Rc::downgrade(&generator));
        module_map.gc_cycle_candidate_allocations =
            module_map.gc_cycle_candidate_allocations.saturating_add(1);
        Ok(Value::Generator(generator))
    }

    /// Run a generator's parameter binding, before the generator object exists.
    ///
    /// A frame of its own, because the binding code is compiled against the callee's slots and
    /// reads the callee's `this`, `new.target` and arguments. The frame is kept rather than
    /// returned from: the locals it writes are the ones the generator resumes with, and a
    /// `Return` would restore the caller's over them.
    pub(crate) fn run_prologue(
        &mut self,
        closure: &Rc<Closure>,
        prologue_end: usize,
        locals: &mut Vec<Value>,
        this_value: &Value,
        new_target: &Value,
        arguments: &[Value],
    ) -> Result<(), ExecutionError> {
        // This re-enters the run loop, so it costs a Rust frame exactly as a built-in calling
        // back into JavaScript does, and it is bounded by the same budget for the same reason:
        // a generator whose parameter default builds another generator recurses here.
        if self.native_depth >= self.limits.max_native_depth {
            return Err(ExecutionError::CallDepthExceeded {
                limit: self.limits.max_native_depth,
            });
        }
        let stack_base = self.stack.len();
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
            method_name: std::mem::replace(&mut self.method_name, None),
            this_value: std::mem::replace(&mut self.this_value, this_value.clone()),
            new_target: std::mem::replace(&mut self.new_target, new_target.clone()),
            arguments: std::mem::replace(&mut self.arguments, arguments.to_vec()),
            arguments_object: self.arguments_object.take(),
            mapped_arguments: self.mapped_arguments,
            constructing: None,
            super_call: false,
            locals: std::mem::replace(&mut self.locals, std::mem::take(locals)),
            // The binding reads the module scope its own function was written in, which is the
            // caller's here for the same reason an ordinary call's is.
            global_base: self.global_base,
            instruction: self.instruction,
            stack_base,
        });
        self.function = Some(Rc::clone(closure));
        self.mapped_arguments = closure.has_mapped_arguments();
        self.instruction = 0;
        let depth = self.frames.len();
        // A throw here belongs to a handler outside this nested run loop, and jumping to one
        // from inside it would carry on executing the caller's `catch` in a loop that thinks it
        // is still binding parameters. The depth bump is what makes `unwind` refuse the jump and
        // send the value out as `ThrownAcrossNative`, for the same handler to take once the Rust
        // stack is flat again.
        self.native_depth += 1;
        // The frame boundary is the safety net: the prologue holds no `Return`, so only the
        // position boundary should ever stop this.
        let outcome = self.run_bounded(depth - 1, prologue_end, depth);
        self.native_depth -= 1;
        // A throw leaves every frame it crossed standing, this one included; whichever handler
        // eventually takes the value pops back through them, restoring each caller exactly as a
        // return would have. Unwinding here as well would pop them twice.
        outcome?;
        *locals = std::mem::take(&mut self.locals);
        self.stack.truncate(stack_base);
        self.restore_caller_frame();
        Ok(())
    }

    pub(crate) fn eval_catching<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, ExecutionError>,
    ) -> Result<Result<T, Value>, ExecutionError> {
        let installed = self.handlers.len();
        self.handlers.push(Handler {
            target: HOST_HANDLER,
            frame_depth: self.frames.len(),
            stack_height: self.stack.len(),
            native_depth: self.native_depth,
        });
        match f(self) {
            Ok(value) => {
                self.handlers.truncate(installed);
                Ok(Ok(value))
            }
            Err(ExecutionError::CaughtByHost) => {
                self.handlers.truncate(installed);
                Ok(Err(self.caught.take().unwrap_or(Value::Undefined)))
            }
            Err(ExecutionError::ThrownAcrossNative) => {
                self.handlers.truncate(installed);
                Ok(Err(self.caught.take().unwrap_or(Value::Undefined)))
            }
            Err(error) => {
                self.handlers.truncate(installed);
                match error.as_javascript_error() {
                    Some((kind, msg)) => {
                        let err = self.construct_error(kind, &[Value::String(msg.into())]);
                        Ok(Err(err))
                    }
                    None => Err(error),
                }
            }
        }
    }

    pub(crate) fn get_method(
        &mut self,
        target: &Value,
        name: &str,
        instruction: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        let method = self.read_property(target, name, instruction)?;
        if method.is_nullish() {
            Ok(None)
        } else if is_callable(&method) {
            Ok(Some(method))
        } else {
            self.throw_error(
                ErrorKind::Type,
                format!("Property '{name}' is not a function"),
                instruction,
            )?;
            Ok(None)
        }
    }

    pub(crate) fn delegate_generator_return(
        &mut self,
        generator: &Rc<GeneratorData>,
        delegate: Value,
        exit_target: usize,
        sent: Value,
        instruction: usize,
    ) -> Result<(Value, bool), ExecutionError> {
        let _ = exit_target;
        let method_res =
            self.eval_catching(|this| this.get_method(&delegate, "return", instruction))?;
        let return_method = match method_res {
            Ok(method) => method,
            Err(thrown) => {
                generator.borrow_mut().delegate = None;
                return self.resume_generator_with(generator, Resume::Throw(thrown), instruction);
            }
        };
        match return_method {
            None => {
                generator.borrow_mut().delegate = None;
                self.resume_generator_return(generator, sent, instruction)
            }
            Some(return_fn) => {
                let call_res = self.call_catching(&return_fn, delegate, vec![sent], instruction)?;
                let inner_result = match call_res {
                    Ok(val) => val,
                    Err(thrown) => {
                        generator.borrow_mut().delegate = None;
                        return self.resume_generator_with(
                            generator,
                            Resume::Throw(thrown),
                            instruction,
                        );
                    }
                };
                if !is_object_type(&inner_result) {
                    generator.borrow_mut().delegate = None;
                    let err = self.construct_error(
                        ErrorKind::Type,
                        &[Value::String(
                            format!(
                                "Iterator result {} is not an object",
                                inner_result.to_display_string()
                            )
                            .into(),
                        )],
                    );
                    return self.resume_generator_with(generator, Resume::Throw(err), instruction);
                }
                let done_res = self
                    .eval_catching(|this| this.read_property(&inner_result, "done", instruction))?;
                let done_val = match done_res {
                    Ok(val) => val,
                    Err(thrown) => {
                        generator.borrow_mut().delegate = None;
                        return self.resume_generator_with(
                            generator,
                            Resume::Throw(thrown),
                            instruction,
                        );
                    }
                };
                let done = done_val.is_truthy();
                if done {
                    generator.borrow_mut().delegate = None;
                    let val_res = self.eval_catching(|this| {
                        this.read_property(&inner_result, "value", instruction)
                    })?;
                    let val = match val_res {
                        Ok(v) => v,
                        Err(thrown) => {
                            return self.resume_generator_with(
                                generator,
                                Resume::Throw(thrown),
                                instruction,
                            );
                        }
                    };
                    self.resume_generator_with(generator, Resume::Return(val), instruction)
                } else {
                    Ok((inner_result, false))
                }
            }
        }
    }

    pub(crate) fn delegate_generator_throw(
        &mut self,
        generator: &Rc<GeneratorData>,
        delegate: Value,
        exit_target: usize,
        thrown: Value,
        instruction: usize,
    ) -> Result<(Value, bool), ExecutionError> {
        let method_res =
            self.eval_catching(|this| this.get_method(&delegate, "throw", instruction))?;
        let throw_method = match method_res {
            Ok(method) => method,
            Err(thrown_val) => {
                generator.borrow_mut().delegate = None;
                return self.resume_generator_with(
                    generator,
                    Resume::Throw(thrown_val),
                    instruction,
                );
            }
        };
        match throw_method {
            None => {
                generator.borrow_mut().delegate = None;
                let return_method_res =
                    self.eval_catching(|this| this.get_method(&delegate, "return", instruction))?;
                match return_method_res {
                    Ok(Some(return_fn)) => {
                        let close_res =
                            self.call_catching(&return_fn, delegate, vec![], instruction)?;
                        match close_res {
                            Ok(inner_res) => {
                                if !is_object_type(&inner_res) {
                                    let err = self.construct_error(
                                        ErrorKind::Type,
                                        &[Value::String(
                                            format!(
                                                "Iterator result {} is not an object",
                                                inner_res.to_display_string()
                                            )
                                            .into(),
                                        )],
                                    );
                                    return self.resume_generator_with(
                                        generator,
                                        Resume::Throw(err),
                                        instruction,
                                    );
                                }
                            }
                            Err(thrown_val) => {
                                return self.resume_generator_with(
                                    generator,
                                    Resume::Throw(thrown_val),
                                    instruction,
                                );
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(thrown_val) => {
                        return self.resume_generator_with(
                            generator,
                            Resume::Throw(thrown_val),
                            instruction,
                        );
                    }
                }
                let err = self.construct_error(
                    ErrorKind::Type,
                    &[Value::String(
                        "Iterator does not have a throw method".to_owned().into(),
                    )],
                );
                self.resume_generator_with(generator, Resume::Throw(err), instruction)
            }
            Some(throw_fn) => {
                let call_res =
                    self.call_catching(&throw_fn, delegate, vec![thrown], instruction)?;
                let inner_result = match call_res {
                    Ok(val) => val,
                    Err(thrown_val) => {
                        generator.borrow_mut().delegate = None;
                        return self.resume_generator_with(
                            generator,
                            Resume::Throw(thrown_val),
                            instruction,
                        );
                    }
                };
                if !is_object_type(&inner_result) {
                    generator.borrow_mut().delegate = None;
                    let err = self.construct_error(
                        ErrorKind::Type,
                        &[Value::String(
                            format!(
                                "Iterator result {} is not an object",
                                inner_result.to_display_string()
                            )
                            .into(),
                        )],
                    );
                    return self.resume_generator_with(generator, Resume::Throw(err), instruction);
                }
                let done_res = self
                    .eval_catching(|this| this.read_property(&inner_result, "done", instruction))?;
                let done_val = match done_res {
                    Ok(val) => val,
                    Err(thrown_val) => {
                        generator.borrow_mut().delegate = None;
                        return self.resume_generator_with(
                            generator,
                            Resume::Throw(thrown_val),
                            instruction,
                        );
                    }
                };
                let done = done_val.is_truthy();
                if done {
                    generator.borrow_mut().delegate = None;
                    let val_res = self.eval_catching(|this| {
                        this.read_property(&inner_result, "value", instruction)
                    })?;
                    let val = match val_res {
                        Ok(v) => v,
                        Err(thrown_val) => {
                            return self.resume_generator_with(
                                generator,
                                Resume::Throw(thrown_val),
                                instruction,
                            );
                        }
                    };
                    generator.borrow_mut().instruction = exit_target;
                    self.resume_generator_with(generator, Resume::Send(val), instruction)
                } else {
                    Ok((inner_result, false))
                }
            }
        }
    }

    /// Start or resume a generator, returning its next value and whether it finished.
    pub(crate) fn resume_generator(
        &mut self,
        generator: &Rc<GeneratorData>,
        sent: Value,
        instruction: usize,
    ) -> Result<(Value, bool), ExecutionError> {
        self.resume_generator_with(generator, Resume::Send(sent), instruction)
    }

    /// Resume a generator for a public `.return()` call. Return completion is represented by an
    /// internal signal while `finally` blocks unwind, so it must pass through the host boundary
    /// catcher just like the callback that drives it. Calling `resume_generator_with` directly
    /// lets that marker escape as the observable iteration value when a delegated iterator has a
    /// `finally`.
    pub(crate) fn resume_generator_return(
        &mut self,
        generator: &Rc<GeneratorData>,
        sent: Value,
        instruction: usize,
    ) -> Result<(Value, bool), ExecutionError> {
        match self.resume_catching(generator, Resume::Return(sent), instruction)? {
            Ok(result) => Ok(result),
            Err(thrown) => {
                self.caught = Some(thrown);
                Err(ExecutionError::ThrownAcrossNative)
            }
        }
    }

    /// Resume a generator, optionally by throwing the `.return()` signal into it.
    pub(crate) fn resume_generator_with(
        &mut self,
        generator: &Rc<GeneratorData>,
        resume: Resume,
        instruction: usize,
    ) -> Result<(Value, bool), ExecutionError> {
        if generator.borrow().done {
            return Ok((Value::Undefined, true));
        }
        // Resuming a generator from inside its own body is a TypeError. Letting it through
        // recursed until the *Rust* stack overflowed, and a stack overflow aborts the process
        // rather than unwinding -- so one such test reported nothing for its whole area.
        if generator.borrow().running {
            return Err(ExecutionError::GeneratorAlreadyRunning);
        }
        // A resumption re-enters the run loop, so it costs a Rust frame exactly as a built-in
        // calling back into JavaScript does, and it is bounded by the same budget for the same
        // reason: `max_call_depth` counts frames in a vector that can grow, this counts a stack
        // that cannot and whose exhaustion aborts rather than throwing. Mutual recursion through
        // a generator -- a body that calls a function which resumes another generator -- reaches
        // no other limit, and overflowed the stack even with a gigabyte of it.
        if self.native_depth >= self.limits.max_native_depth {
            return Err(ExecutionError::CallDepthExceeded {
                limit: self.limits.max_native_depth,
            });
        }
        generator.borrow_mut().running = true;
        let _running = RunningGuard(generator);
        self.native_depth += 1;
        let outcome = self.resume_generator_body(generator, resume, instruction);
        self.native_depth -= 1;
        outcome
    }

    /// One resumption, with the guards already taken.
    pub(crate) fn resume_generator_body(
        &mut self,
        generator: &Rc<GeneratorData>,
        resume: Resume,
        instruction: usize,
    ) -> Result<(Value, bool), ExecutionError> {
        let sent = match &resume {
            Resume::Send(value) | Resume::Throw(value) | Resume::Return(value) => value.clone(),
        };
        let raising = !matches!(resume, Resume::Send(_));
        let depth = self.frames.len();
        let stack_base = self.stack.len();
        let (
            closure,
            locals,
            saved_stack,
            saved_handlers,
            instruction_pointer,
            this_value,
            new_target,
            arguments,
            started,
        ) = {
            let state = generator.borrow();
            (
                Rc::clone(&state.closure),
                state.locals.clone(),
                state.stack.clone(),
                state.handlers.clone(),
                state.instruction,
                state.this_value.clone(),
                state.new_target.clone(),
                state.arguments.clone(),
                state.started,
            )
        };

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
            method_name: std::mem::replace(&mut self.method_name, None),
            this_value: std::mem::replace(&mut self.this_value, this_value),
            new_target: std::mem::replace(&mut self.new_target, new_target),
            arguments: std::mem::replace(&mut self.arguments, arguments),
            arguments_object: self.arguments_object.take(),
            mapped_arguments: self.mapped_arguments,
            constructing: None,
            super_call: false,
            locals: std::mem::replace(&mut self.locals, locals),
            // A generator body belongs to the module that wrote it, and so does the module scope
            // it reads -- which is the caller's here for the same reason an ordinary call's is.
            global_base: self.global_base,
            instruction: self.instruction,
            stack_base,
        });
        self.function = Some(closure);
        self.mapped_arguments = self
            .function
            .as_ref()
            .is_some_and(|closure| closure.has_mapped_arguments());
        self.instruction = instruction_pointer;
        self.stack.extend(saved_stack);
        // Re-base the generator's handlers onto wherever its stack landed this time.
        for (target, frame_offset, stack_offset) in saved_handlers {
            self.handlers.push(Handler {
                target,
                frame_depth: depth + 1 + frame_offset,
                stack_height: stack_base + stack_offset,
                native_depth: self.native_depth,
            });
        }
        if started && !raising {
            // The value the caller sent becomes the result of the `yield` that suspended. A
            // `.return()` does not resume the yield at all, so it pushes nothing.
            self.stack.push(sent.clone());
        }

        self.suspended = false;
        self.suspended_delegate = None;
        let outcome = if raising {
            // Throwing at the suspension point runs every `finally` between there and the top of
            // the generator, then arrives as `GeneratorReturned`.
            let raised = match &resume {
                Resume::Return(_) => Value::Native(Native::ReturnSignal, RealmId::PRIMARY),
                // A rejection is raised as itself, so the generator's own `catch` can see it.
                other => match other {
                    Resume::Throw(value) => value.clone(),
                    Resume::Send(_) => unreachable!("checked by `raising`"),
                    Resume::Return(_) => unreachable!("matched above"),
                },
            };
            match self
                .unwind(raised, instruction)
                .and_then(|()| self.run_frames(depth))
            {
                // The signal ran out of handlers *inside* the generator, which is what finishing
                // it looks like.
                Err(ExecutionError::GeneratorReturned) => {
                    // The generator's own operand stack is abandoned along with it; without
                    // this the caller inherits whatever it had part-way through an expression.
                    self.stack.truncate(stack_base);
                    self.restore_caller_frame();
                    generator.borrow_mut().complete();
                    return Ok((sent.clone(), true));
                }
                // The same ending, reached the other way: the innermost handler belongs to
                // whoever called `return`, so `unwind` refused to jump to it and sent the signal
                // out as a value instead. It is not one -- it is this generator finishing -- and
                // handing it to that handler put the marker itself in a `catch` binding. The
                // symptom was a program that caught `[Function: returnSignal]` from an ordinary
                // `for (const x of gen) { break; }` written inside a `try`.
                Err(ExecutionError::ThrownAcrossNative)
                    if matches!(self.caught, Some(Value::Native(Native::ReturnSignal, _))) =>
                {
                    self.caught = None;
                    self.stack.truncate(stack_base);
                    self.restore_caller_frame();
                    generator.borrow_mut().complete();
                    return Ok((sent.clone(), true));
                }
                // Any other abrupt completion leaves the body for good. The specification
                // sets the generator to `completed` before it propagates the throw, so a later
                // `next()` answers `{value: undefined, done: true}` rather than re-entering a
                // body that has already unwound -- which ran it a second time from the top.
                Err(error) => {
                    generator.borrow_mut().complete();
                    return Err(error);
                }
                Ok(outcome) => outcome,
            }
        } else {
            match self.run_frames(depth) {
                Ok(outcome) => outcome,
                // Same ending, reached from an ordinary `next()`: a throw out of the body is an
                // abrupt completion, and a completed generator is finished.
                Err(error) => {
                    generator.borrow_mut().complete();
                    return Err(error);
                }
            }
        };

        if self.suspended {
            // Take back the handlers belonging to the generator, as offsets.
            let mut retained = Vec::new();
            while self
                .handlers
                .last()
                .is_some_and(|handler| handler.frame_depth > depth)
            {
                let handler = self.handlers.pop().expect("checked by the loop condition");
                retained.push((
                    handler.target,
                    handler.frame_depth - depth - 1,
                    handler.stack_height.saturating_sub(stack_base),
                ));
            }
            retained.reverse();

            let mut state = generator.borrow_mut();
            state.locals = std::mem::take(&mut self.locals);
            state.instruction = self.instruction;
            state.stack = self.stack.split_off(stack_base);
            state.handlers = retained;
            state.started = true;
            state.awaiting = self.suspended_awaiting;
            state.delegate = self.suspended_delegate.take();
            drop(state);

            self.restore_caller_frame();
            self.suspended = false;
            return Ok((outcome.value, false));
        }

        // A normal return finished the generator; `run_frames` already restored the caller.
        generator.borrow_mut().complete();
        let _ = instruction;
        Ok((outcome.value, true))
    }

    /// One built-in accessor, run against the receiver it was reached through.
    ///
    /// Every one of them is branded: the specification's first step checks the receiver has the
    /// internal slot the accessor reports on, and answers a `TypeError` when it does not. That
    /// check is the whole reason these are accessors rather than table entries -- a getter taken
    /// off `ArrayBuffer.prototype` and called on a plain object has to say so.
    pub(crate) fn builtin_accessor(
        &mut self,
        accessor: BuiltinAccessor,
        receiver: &Value,
        _instruction: usize,
    ) -> Result<Value, ExecutionError> {
        // `Symbol.species` is the exception: it reports its receiver rather than anything about
        // it, and is defined for any receiver at all.
        if matches!(accessor, BuiltinAccessor::Species) {
            return Ok(receiver.clone());
        }
        let branded = match accessor {
            BuiltinAccessor::BufferByteLength
            | BuiltinAccessor::BufferMaxByteLength
            | BuiltinAccessor::BufferResizable
            | BuiltinAccessor::BufferDetached
            | BuiltinAccessor::BufferImmutable => {
                matches!(receiver, Value::ArrayBuffer(buffer) if !buffer.shared)
            }
            BuiltinAccessor::SharedByteLength
            | BuiltinAccessor::SharedMaxByteLength
            | BuiltinAccessor::SharedGrowable => {
                matches!(receiver, Value::ArrayBuffer(buffer) if buffer.shared)
            }
            BuiltinAccessor::DisposableStackDisposed
            | BuiltinAccessor::AsyncDisposableStackDisposed => {
                let flavour = if accessor == BuiltinAccessor::DisposableStackDisposed {
                    Disposal::Sync
                } else {
                    Disposal::Async
                };
                return match receiver {
                    Value::Object(object)
                        if object.borrow().get_own(flavour.entries_key()).is_some() =>
                    {
                        Ok(Value::Boolean(Self::disposable_is_disposed(
                            receiver, flavour,
                        )))
                    }
                    _ => Err(ExecutionError::TypeErrorMessage {
                        message: format!(
                            "{}.prototype.disposed called on an incompatible receiver",
                            flavour.type_name()
                        ),
                    }),
                };
            }
            BuiltinAccessor::DataViewBuffer
            | BuiltinAccessor::DataViewByteLength
            | BuiltinAccessor::DataViewByteOffset => matches!(receiver, Value::DataView(_)),
            BuiltinAccessor::TypedArrayBuffer
            | BuiltinAccessor::TypedArrayByteLength
            | BuiltinAccessor::TypedArrayByteOffset
            | BuiltinAccessor::TypedArrayLength => matches!(receiver, Value::TypedArray(_)),
            // `thisSymbolValue`: a symbol, or a symbol in a wrapper.
            BuiltinAccessor::SymbolDescription => {
                matches!(receiver, Value::Symbol(_))
                    || matches!(wrapped_primitive(receiver), Some(Value::Symbol(_)))
            }
            BuiltinAccessor::CollectionSize(is_map) => {
                !is_weak_collection(receiver)
                    && collection_state(receiver)
                        .is_some_and(|(_, receiver_is_map)| receiver_is_map == is_map)
            }
            BuiltinAccessor::Species => unreachable!("answered above"),
        };
        if !branded {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!(
                    "get {}: receiver does not have the required internal slot",
                    accessor.property_key()
                ),
            });
        }
        // A collection's count lives in a reserved slot rather than under its own name, so that
        // it is not an own property of every instance -- which is what
        // `Object.getOwnPropertyNames(new Map())` reported before this accessor existed.
        // A symbol's description is a slot on the symbol, so a *wrapper* has to be opened to
        // reach it: reading `description` off the wrapper object finds nothing, and answering
        // `undefined` there is indistinguishable from a symbol that genuinely has none.
        if matches!(accessor, BuiltinAccessor::SymbolDescription) {
            let symbol = match receiver {
                Value::Symbol(_) => receiver.clone(),
                other => wrapped_primitive(other).unwrap_or(Value::Undefined),
            };
            return Ok(builtin_accessor_slot(accessor, &symbol));
        }
        if matches!(accessor, BuiltinAccessor::CollectionSize(_)) {
            let Value::Object(object) = receiver else {
                unreachable!("the brand check passed, so this is a collection object");
            };
            let size = object.borrow().get_own(COLLECTION_SIZE).cloned();
            return Ok(size.unwrap_or(Value::Number(0.0)));
        }
        // DataView's two measurements reject a view stranded by resize or detach. Keep that
        // check here because the direct slot read below bypasses public property lookup;
        // otherwise a materialized prototype accessor would dispatch back into this function.
        if matches!(
            accessor,
            BuiltinAccessor::DataViewByteLength | BuiltinAccessor::DataViewByteOffset
        ) && matches!(receiver, Value::DataView(view) if view.out_of_bounds())
        {
            return Err(ExecutionError::DataViewOutOfBounds);
        }
        // Everything else is the same answer an instance gives for the same name, taken from
        // the one place that knows it -- including the `TypeError` a `DataView` a shrink has
        // stranded raises for its two measurements.
        Ok(builtin_accessor_slot(accessor, receiver))
    }

    /// The specification's `InternalizeJSONProperty`: one node of the parsed document, revived.
    ///
    /// Depth first, because a reviver is defined to see its children already revived -- that is
    /// the whole reason it is useful. A child revived to `undefined` is *deleted* rather than
    /// stored, which is how a reviver drops a key.
    ///
    /// The recursion is bounded by the same budget every other re-entry into the machine uses:
    /// this walks a structure the program controls, and the Rust stack it walks on cannot grow.
    pub(crate) fn internalize_json_property(
        &mut self,
        holder: &Value,
        name: &StringUnits,
        reviver: &Value,
        instruction: usize,
        sources: &JsonSourceMap,
    ) -> Result<Value, ExecutionError> {
        if self.native_depth >= self.limits.max_native_depth {
            return Err(ExecutionError::CallDepthExceeded {
                limit: self.limits.max_native_depth,
            });
        }
        let value = self.read_property_key(holder, PropertyKeyRef::Units(name), instruction)?;
        if is_reflect_target(&value) {
            // An array is walked by index up to its own `length`, and anything else by its own
            // enumerable keys. Both readings go through the machine rather than through a store,
            // because either may be a proxy: `length` can be a getter that throws, and the keys
            // come from an `ownKeys` trap.
            let keys: Vec<StringUnits> = if matches!(unwrap_proxy(&value), Value::Array(_)) {
                let length = self.read_property(&value, "length", instruction)?;
                let length = to_length(self.as_number_argument(Some(&length), instruction)?);
                (0..length)
                    .map(|index| StringUnits::from(index.to_string()))
                    .collect()
            } else {
                let mut names: Vec<StringUnits> = Vec::new();
                for key in self.own_keys(&value, instruction)? {
                    let descriptor =
                        self.proxy_descriptor(&value, &Value::String(key.clone()), instruction)?;
                    if is_reflect_target(&descriptor)
                        && self
                            .read_property(&descriptor, "enumerable", instruction)?
                            .is_truthy()
                    {
                        names.push(key);
                    }
                }
                names
            };
            for key in keys {
                self.native_depth += 1;
                let revived =
                    self.internalize_json_property(&value, &key, reviver, instruction, sources);
                self.native_depth -= 1;
                let revived = revived?;
                if matches!(revived, Value::Undefined) {
                    self.delete(&value, &Value::String(key.clone()), instruction)?;
                } else {
                    // `CreateDataProperty`, not a write: a reviver's result is installed as a
                    // plain own property whatever the holder had there, and a holder that
                    // refuses the definition -- a non-configurable slot, a proxy trap saying no
                    // -- is *ignored* rather than made into an error. `Reflect.defineProperty`
                    // is the spelling of the operation that reports rather than throws.
                    let mut descriptor = ObjectData::default();
                    descriptor.set("value", revived);
                    descriptor.set("writable", Value::Boolean(true));
                    descriptor.set("enumerable", Value::Boolean(true));
                    descriptor.set("configurable", Value::Boolean(true));
                    self.dispatch_native(
                        Native::ReflectDefineProperty,
                        &Value::Undefined,
                        &[
                            value.clone(),
                            Value::String(key),
                            Value::Object(tracked_object(descriptor)),
                        ],
                        instruction,
                    )?;
                }
            }
        }
        let holder_identity = match holder {
            Value::Object(object) => Rc::as_ptr(object) as usize,
            Value::Array(array) => Rc::as_ptr(array) as usize,
            _ => 0,
        };
        let mut context = ObjectData::default();
        if let Some((expected, raw)) = sources.get(&(holder_identity, name.clone()))
            && same_value(&value, expected)
        {
            context.set("source", Value::String(raw.clone()));
        }
        self.call_value(
            reviver,
            holder.clone(),
            vec![
                Value::String(name.clone()),
                value,
                Value::Object(tracked_object(context)),
            ],
            instruction,
        )
    }

    /// Put back the frame a generator resumption displaced.
    pub(crate) fn restore_caller_frame(&mut self) {
        let frame = self
            .frames
            .pop()
            .expect("a resumption always pushes a frame");
        self.locals = frame.locals;
        self.program = frame.program;
        self.strictness = frame.strictness;
        self.current_realm = frame.realm;
        self.strict = frame.strict;
        self.module_path = frame.module_path;
        self.module_generation = frame.module_generation;
        self.module_generation_owner = frame.module_generation_owner;
        self.function = frame.function;
        self.method_name = frame.method_name;
        self.this_value = frame.this_value;
        self.new_target = frame.new_target;
        self.arguments = frame.arguments;
        self.arguments_object = frame.arguments_object;
        self.mapped_arguments = frame.mapped_arguments;
        self.global_base = frame.global_base;
        self.instruction = frame.instruction;
    }

    /// Collect every value an iterable yields.
    ///
    /// Drives the same protocol `for-of` uses rather than a second one, so anything iterable in
    /// a loop is iterable in a spread, and a value that is not iterable fails the same way.
    /// Invoke a native, including the few that need the interpreter itself.
    ///
    /// `call_native` is a free function so that the great majority of built-ins -- pure
    /// transformations of their arguments -- stay testable without a VM around them. The handful
    /// that drive the machine, by walking the iterator protocol or calling back into user code,
    /// cannot live there. They are peeled off here, and every call site goes through this, which
    /// is what stops the two halves from disagreeing about which native is which.
    /// The specification's `ToPropertyDescriptor`: the object a program writes, read into the six
    /// optional fields the definition algorithm compares against.
    ///
    /// Every field is read through the property machinery rather than out of a store, because a
    /// descriptor may itself be an object with getters -- and how many times each of those runs,
    /// and in what order, is observable.
    pub(crate) fn read_descriptor(
        &mut self,
        source: &Value,
        instruction: usize,
    ) -> Result<Descriptor, ExecutionError> {
        if !is_an_object(source) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!(
                    "Property description must be an object: {}",
                    source.to_display_string()
                ),
            });
        }
        let mut descriptor = Descriptor::default();
        // The specification's order, and it is observable through a descriptor whose fields are
        // getters that record when they ran.
        for field in [
            "enumerable",
            "configurable",
            "value",
            "writable",
            "get",
            "set",
        ] {
            if !has_property_checked(
                &self.intrinsics,
                source,
                &Value::String(field.to_owned().into()),
            )? {
                continue;
            }
            let read = self.read_property(source, field, instruction)?;
            match field {
                "enumerable" => descriptor.enumerable = Some(read.is_truthy()),
                "configurable" => descriptor.configurable = Some(read.is_truthy()),
                "writable" => descriptor.writable = Some(read.is_truthy()),
                "value" => descriptor.value = Some(read),
                _ => {
                    // `undefined` is the one non-function an accessor half may be: it names the
                    // half as absent, which is how a getter-only property is written.
                    if !is_callable(&read) && !matches!(read, Value::Undefined) {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: format!("{field}ter must be a function"),
                        });
                    }
                    if field == "get" {
                        descriptor.get = Some(read);
                    } else {
                        descriptor.set = Some(read);
                    }
                }
            }
        }
        if descriptor.is_accessor() && descriptor.is_data() {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Invalid property descriptor. Cannot both specify accessors and a value \
                          or writable attribute"
                    .to_owned(),
            });
        }
        Ok(descriptor)
    }

    /// `ObjectDefineProperties`: read and validate every descriptor, then apply them.
    ///
    /// The two passes are the specification's and they are not cosmetic: a descriptor that is
    /// not an object, halfway down the list, has to leave the target exactly as it found it
    /// rather than half-defined.
    pub(crate) fn define_properties(
        &mut self,
        target: &Value,
        source: &Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        // `ToObject` of the descriptor map. A primitive coerces to a wrapper with no own
        // enumerable properties, so it defines nothing; `null` and `undefined` cannot coerce.
        if source.is_nullish() {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Cannot convert undefined or null to object".to_owned(),
            });
        }
        let mut descriptors = Vec::new();
        // `Object.defineProperties` consumes `[[OwnPropertyKeys]]`, not
        // `Object.getOwnPropertyNames`: symbol descriptors participate too. The two halves
        // keep the wrapper their domain needs for `matches_slot` -- `Units` for names,
        // `SymbolUnits` for reserved symbol slots -- in the specification's own order,
        // strings then symbols, so one loop serves both without re-resolving any key's
        // domain from its spelling.
        let string_keys = own_property_keys_units(&self.intrinsics, source);
        let symbol_keys = own_symbol_keys_units(&self.intrinsics, source);
        let keys = string_keys
            .iter()
            .map(|key| ((*key).clone(), PropertyKeyRef::Units(key)))
            .chain(
                symbol_keys
                    .iter()
                    .map(|key| ((*key).clone(), PropertyKeyRef::SymbolUnits(key))),
            );
        for (key, key_ref) in keys {
            if !own_attributes_key(&self.intrinsics, source, key_ref)
                .is_some_and(|attributes| attributes.enumerable)
            {
                continue;
            }
            let read = self.read_property_key(source, key_ref, instruction)?;
            let descriptor = self.read_descriptor(&read, instruction)?;
            descriptors.push((key, key_ref, descriptor));
        }
        for (key, key_ref, descriptor) in descriptors {
            // `DefinePropertyOrThrow` applies the internal descriptor record, so a proxy sees a
            // fresh `FromPropertyDescriptor` object rather than the source descriptor. The trap
            // names its key as the language does: a reserved spelling resolves back into the symbol,
            // mirroring `own_keys_reflected`, so a symbol descriptor arrives as the symbol
            // rather than `"@@symbol:N"`. String keys pass through unchanged, and an
            // unresolvable spelling keeps the string it already had rather than inventing a key.
            if matches!(target, Value::Proxy(_)) {
                let trap_arg = match key_ref {
                    PropertyKeyRef::SymbolUnits(units) => self
                        .symbol_of_key(&units.to_lossy_string())
                        .map(Value::Symbol)
                        .unwrap_or_else(|| Value::String(key.clone())),
                    _ => Value::String(key.clone()),
                };
                self.dispatch_native(
                    Native::ObjectDefineProperty,
                    &Value::Undefined,
                    &[
                        target.clone(),
                        trap_arg,
                        Self::descriptor_object(&descriptor),
                    ],
                    instruction,
                )?;
                continue;
            }
            if !self.define_own_key(target, key_ref, &descriptor, instruction)? {
                return Err(ExecutionError::TypeErrorMessage {
                    message: refusal(&self.intrinsics, target, &key.to_lossy_string()),
                });
            }
        }
        Ok(())
    }
}
