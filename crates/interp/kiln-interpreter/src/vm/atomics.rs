use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// One `Atomics` operation.
    ///
    /// On the machine rather than in [`call_native`] because the order the arguments are
    /// converted in is observable and the corpus checks every step of it: the view is validated
    /// before the index is coerced, the index before the value, and the buffer is looked at
    /// *again* afterwards because a program's `valueOf` can detach it in between.
    pub(super) fn atomics(
        &mut self,
        op: AtomicsOp,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        match op {
            // Neither of these touches a view. `isLockFree` answers for a *width*, and the
            // answer has to be the same every time a program asks, which is the only thing the
            // specification requires of it.
            AtomicsOp::IsLockFree => {
                let size = self.as_number_argument(arguments.first(), instruction)?;
                Ok(Value::Boolean(matches!(size, 1.0 | 2.0 | 4.0 | 8.0)))
            }
            // A hint to the processor that a spin-wait is in progress. There is nothing to hint
            // to here, so the whole of it is the argument check: an iteration count has to be an
            // integral Number if it is there at all.
            AtomicsOp::Pause => {
                match arguments.first() {
                    None | Some(Value::Undefined) => {}
                    Some(Value::Number(count)) if count.fract() == 0.0 && count.is_finite() => {}
                    Some(_) => {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: "Atomics.pause: iterationNumber must be an integer".to_owned(),
                        });
                    }
                }
                Ok(Value::Undefined)
            }
            AtomicsOp::Wait | AtomicsOp::WaitAsync => self.atomics_wait(op, arguments, instruction),
            AtomicsOp::Notify => self.atomics_notify(arguments, instruction),
            _ => self.atomics_access(op, arguments, instruction),
        }
    }

    /// `ValidateIntegerTypedArray`: the view an atomic operation may be applied to.
    ///
    /// A float view is refused because an atomic add on one is not defined, and a clamped one
    /// because its writes saturate rather than wrap -- neither is an *unclamped integer* element
    /// type. A waiting operation narrows that further to the two widths the specification gives
    /// a waiter list for.
    pub(super) fn atomics_view(
        &self,
        arguments: &[Value],
        op: AtomicsOp,
    ) -> Result<Rc<TypedArrayData>, ExecutionError> {
        let Some(Value::TypedArray(view)) = arguments.first() else {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Atomics operations require a typed array".to_owned(),
            });
        };
        if view.out_of_bounds() {
            return Err(ExecutionError::BufferDetached);
        }
        // Every atomic operation but `load` writes, and the two that do not are refused with
        // them: the specification excludes an immutable buffer from atomic access outright,
        // because an atomic read of bytes nothing can change is a lock nobody needs.
        if view.buffer.immutable {
            return Err(ExecutionError::BufferImmutable);
        }
        let allowed = if op.waitable() {
            matches!(view.kind, TypedKind::Int32 | TypedKind::BigInt64)
        } else {
            // `ValidateIntegerTypedArray` names the types atomics accept, and every float is
            // outside it -- half included. `Float16Array` was missing here only because the
            // conformance harness's `typeof Float16Array` guard could not see the constructor
            // this realm registers, so no test ever reached the check.
            !matches!(
                view.kind,
                TypedKind::Uint8Clamped
                    | TypedKind::Float16
                    | TypedKind::Float32
                    | TypedKind::Float64
            )
        };
        if !allowed {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!(
                    "Atomics.{} cannot be applied to a {}",
                    op.property_key(),
                    view.kind.name()
                ),
            });
        }
        Ok(Rc::clone(view))
    }

    /// `ValidateAtomicAccess`: the element an index reaches, or the `RangeError` it does not.
    ///
    /// `ToIndex` first -- which can run a program's `valueOf` -- and only then the bounds check,
    /// because a program watching that conversion can see which of the two refused. The length
    /// the check uses is the one read *before* the conversion: a `valueOf` that grows the buffer
    /// does not widen the range of indices this call may reach, which is what makes the bound a
    /// property of the operation rather than a race with it.
    pub(super) fn atomics_index(
        &mut self,
        view: &Rc<TypedArrayData>,
        requested: Option<&Value>,
        instruction: usize,
    ) -> Result<usize, ExecutionError> {
        let length = view.len();
        let number = self.as_number_argument(requested, instruction)?;
        let integer = if number.is_nan() { 0.0 } else { number.trunc() };
        if !(0.0..=9_007_199_254_740_991.0).contains(&integer) {
            return Err(ExecutionError::RangeErrorMessage {
                message: format!("Invalid atomic access index {number}"),
            });
        }
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let index = integer as usize;
        if index >= length {
            return Err(ExecutionError::RangeErrorMessage {
                message: format!("Invalid atomic access index {index}"),
            });
        }
        Ok(index)
    }

    /// The operand of an atomic operation, in the view's own numeric tower.
    ///
    /// `ToIntegerOrInfinity` rather than `ToNumber` for a Number view: `Atomics.add(i32, 0, 1.5)`
    /// adds one. The truncation happens here rather than in the write because `store` hands the
    /// *converted* value back, which is how `Atomics.store(i32, 0, Infinity)` returns `Infinity`
    /// after writing a zero.
    pub(super) fn atomics_operand(
        &mut self,
        kind: TypedKind,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if kind.is_big() {
            return Ok(Value::BigInt(Rc::new(
                self.as_big_int_argument(value, instruction)?,
            )));
        }
        let number = self.as_number_argument(value, instruction)?;
        Ok(Value::Number(Self::to_integer_or_infinity(number)))
    }

    /// A read-modify-write, a load, or a store: everything that reaches one element.
    pub(super) fn atomics_access(
        &mut self,
        op: AtomicsOp,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let view = self.atomics_view(arguments, op)?;
        let index = self.atomics_index(&view, arguments.get(1), instruction)?;
        let operand = match op {
            AtomicsOp::Load => Value::Undefined,
            _ => self.atomics_operand(view.kind, arguments.get(2), instruction)?,
        };
        // `compareExchange` converts both of its values, in the order it is given them, and only
        // then looks at the buffer again.
        let replacement = match op {
            AtomicsOp::CompareExchange => {
                Some(self.atomics_operand(view.kind, arguments.get(3), instruction)?)
            }
            _ => None,
        };
        // Read again, after every conversion: a `valueOf` above may have detached the buffer or
        // shrunk it under the view, and the specification revalidates rather than writing into
        // memory that is no longer there.
        if view.out_of_bounds() || index >= view.len() {
            return Err(ExecutionError::BufferDetached);
        }
        let old = view.get(index).unwrap_or(Value::Undefined);
        match op {
            AtomicsOp::Load => Ok(old),
            AtomicsOp::Store => {
                view.set(index, &operand)?;
                // The value the *conversion* produced, not the one the buffer now holds: they
                // differ whenever the write wrapped, and the specification returns the former.
                Ok(operand)
            }
            AtomicsOp::CompareExchange => {
                let expected = narrow_element(view.kind, &operand);
                if same_value_zero(&old, &expected) {
                    view.set(index, &replacement.unwrap_or(Value::Undefined))?;
                }
                Ok(old)
            }
            _ => {
                let combined = atomics_combine(view.kind, op, &old, &operand);
                view.set(index, &combined)?;
                Ok(old)
            }
        }
    }

    /// `Atomics.wait` and `Atomics.waitAsync`.
    ///
    /// This realm has exactly one agent, so nothing can ever reach the waiter list while this
    /// one is sitting on it: a wait that is not answered "not-equal" straight away always ends
    /// in "timed-out". The synchronous spelling therefore sleeps out its timeout and the
    /// asynchronous one schedules the answer, which is what a program can observe of either.
    pub(super) fn atomics_wait(
        &mut self,
        op: AtomicsOp,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let view = self.atomics_view(arguments, op)?;
        if !view.buffer.shared {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Atomics.wait requires a SharedArrayBuffer".to_owned(),
            });
        }
        let index = self.atomics_index(&view, arguments.get(1), instruction)?;
        let expected = self.atomics_operand(view.kind, arguments.get(2), instruction)?;
        let timeout = self.as_number_argument(arguments.get(3), instruction)?;
        // `undefined` and `NaN` both mean "no limit"; anything else is clamped up to zero.
        let timeout = if timeout.is_nan() {
            f64::INFINITY
        } else {
            timeout.max(0.0)
        };
        if view.out_of_bounds() || index >= view.len() {
            return Err(ExecutionError::BufferDetached);
        }
        let held = view.get(index).unwrap_or(Value::Undefined);
        let answer = if same_value_zero(&held, &narrow_element(view.kind, &expected)) {
            None
        } else {
            Some("not-equal")
        };
        if op == AtomicsOp::Wait {
            let settled = match answer {
                Some(answer) => answer.to_owned(),
                None => self.atomics_block(&view, index, timeout)?,
            };
            return Ok(Value::String(settled.into()));
        }
        // `waitAsync` reports whether it had to wait at all, beside either the answer itself or
        // the promise that will carry it.
        let (asynchronous, value) = match answer {
            Some(answer) => (false, Value::String(answer.to_owned().into())),
            None if timeout == 0.0 => (false, Value::String("timed-out".to_owned().into())),
            None => {
                let promise = PromiseState::pending();
                let store = view.buffer.bytes.store().map(Arc::clone);
                match (self.agents.clone(), store) {
                    // A real waiter, on the same list the synchronous spelling joins: a `notify`
                    // from another agent settles it "ok", and the group's virtual clock settles
                    // it "timed-out" when the deadline arrives. Neither costs wall-clock time.
                    (Some(group), Some(store)) => {
                        let address =
                            agent::address(&store, view.offset + index * view.kind.width());
                        let ticket = group.wait_async(self.agent_id, address, timeout);
                        self.async_waits.push((ticket, Rc::clone(&promise)));
                        // The deadline this wait is on is the scheduler's, and the only thing
                        // that can move the scheduler's clock is a quantum boundary in an agent
                        // -- which for a program with one agent means this one.
                        self.checkpoint = self.next_checkpoint();
                    }
                    // No agents: nothing can ever reach the waiter list, so the wait can only
                    // run out. A realm timer is what says so without hanging.
                    _ => {
                        let timer = Timer {
                            // The resolve half of the promise's own pair, called with the answer:
                            // a timer carries its state in its arguments rather than in a closure.
                            callback: Value::Settler(Rc::clone(&promise), false),
                            arguments: vec![Value::String("timed-out".to_owned().into())],
                            context: Rc::clone(&self.async_context),
                        };
                        let delay = duration_from_milliseconds(if timeout.is_finite() {
                            timeout
                        } else {
                            ATOMICS_UNBOUNDED_WAIT_MS
                        });
                        self.with_realm_mut(|realm| realm.set_timeout(delay, timer));
                    }
                }
                (true, Value::Promise(promise))
            }
        };
        let mut result = ObjectData::default();
        result.set("async", Value::Boolean(asynchronous));
        result.set("value", value);
        Ok(Value::Object(Rc::new(RefCell::new(result))))
    }

    /// Suspend this agent on the cell it is waiting for, and answer with how the wait ended.
    ///
    /// This is the operation `Atomics.wait` exists to be, and until there was a second agent it
    /// could not be written: the value has already been compared against the cell, and no other
    /// agent has run since, because no other agent runs until this one gives the baton up here.
    /// That is the specification's atomic compare-and-block, obtained from the scheduler rather
    /// than from a lock on the bytes.
    ///
    /// A host that granted no agents keeps the old answer, because it is still the true one:
    /// nothing can reach the waiter list, so the wait can only run out.
    pub(super) fn atomics_block(
        &mut self,
        view: &Rc<TypedArrayData>,
        index: usize,
        timeout: f64,
    ) -> Result<String, ExecutionError> {
        let store = view.buffer.bytes.store().map(Arc::clone);
        let (Some(group), Some(store)) = (self.agents.clone(), store) else {
            Self::atomics_sleep(timeout);
            return Ok("timed-out".to_owned());
        };
        let address = agent::address(&store, view.offset + index * view.kind.width());
        let outcome = group
            .wait(self.agent_id, address, timeout)
            .map_err(|_| ExecutionError::AgentTerminated)?;
        // Another agent has run since, so this one's quantum has to start again from here.
        self.checkpoint = self.next_checkpoint();
        Ok(outcome.as_str().to_owned())
    }

    /// Settle every `Atomics.waitAsync` promise whose wait has ended.
    ///
    /// Called wherever this agent could have learnt something: after a `notify` it performed
    /// itself, at every quantum boundary, and at each turn of its event loop. The scheduler
    /// decides *how* a wait ended; this only hands the answer to the promise.
    pub(super) fn settle_async_waits(&mut self) -> Result<bool, ExecutionError> {
        let Some(group) = self.agents.clone() else {
            return Ok(false);
        };
        if self.async_waits.is_empty() {
            return Ok(false);
        }
        let mut settled = false;
        for (ticket, outcome) in group.take_async_settled(self.agent_id) {
            let Some(position) = self
                .async_waits
                .iter()
                .position(|(held, _)| *held == ticket)
            else {
                continue;
            };
            let (_, promise) = self.async_waits.remove(position);
            let settler = Value::Settler(promise, false);
            self.call_value(
                &settler,
                Value::Undefined,
                vec![Value::String(outcome.as_str().to_owned().into())],
                self.instruction,
            )?;
            settled = true;
        }
        Ok(settled)
    }

    /// Sleep out a wait that nothing will ever interrupt.
    ///
    /// A wait with no limit is a deadlock in a realm with one agent: the only code that could
    /// call `notify` is the code that is blocked. Rather than hang the process on a program no
    /// engine could have finished either, the wait is bounded -- a limit of this realm, reached
    /// only by a host that granted no agents at all. A host that did gets the scheduler's answer
    /// instead, which costs no wall-clock time because its clock is virtual.
    pub(super) fn atomics_sleep(timeout: f64) {
        let milliseconds = if timeout.is_finite() {
            timeout.min(ATOMICS_UNBOUNDED_WAIT_MS)
        } else {
            ATOMICS_UNBOUNDED_WAIT_MS
        };
        if milliseconds > 0.0 {
            std::thread::sleep(duration_from_milliseconds(milliseconds));
        }
    }

    /// `Atomics.notify`: how many waiters were woken.
    ///
    /// The count is exactly how many agents were sitting on this cell, and it is the same on
    /// every run: the schedule that put them there is decided by an instruction quantum and a
    /// virtual clock rather than by the machine's mood. Zero when this host granted no agents,
    /// or when the view is over an ordinary buffer, which nothing can wait on.
    pub(super) fn atomics_notify(
        &mut self,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let view = self.atomics_view(arguments, AtomicsOp::Notify)?;
        let index = self.atomics_index(&view, arguments.get(1), instruction)?;
        let _ = index;
        // `ToIntegerOrInfinity`, clamped up to zero, and converted even though nothing waits on
        // the answer: the conversion can run a program's `valueOf`.
        let count = match arguments.get(2) {
            None | Some(Value::Undefined) => f64::INFINITY,
            value => {
                Self::to_integer_or_infinity(self.as_number_argument(value, instruction)?).max(0.0)
            }
        };
        // The conversion above can have run the program's own `valueOf`, and that code may have
        // detached or resized the buffer under this view. A wake into a window that is no longer
        // there wakes nobody, which is what the specification's re-validation amounts to here.
        if view.out_of_bounds() || index >= view.len() {
            return Ok(Value::Number(0.0));
        }
        let store = view.buffer.bytes.store().map(Arc::clone);
        let (Some(group), Some(store)) = (self.agents.clone(), store) else {
            return Ok(Value::Number(0.0));
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let wanted = if count.is_finite() {
            count.min(usize::MAX as f64) as usize
        } else {
            usize::MAX
        };
        let address = agent::address(&store, view.offset + index * view.kind.width());
        let woken = group.notify(address, wanted);
        // An asynchronous waiter this agent woke is one of its own promises, and the program
        // that called `notify` expects it to settle without another turn of anything.
        self.settle_async_waits()?;
        #[allow(clippy::cast_precision_loss)]
        Ok(Value::Number(woken as f64))
    }
}
