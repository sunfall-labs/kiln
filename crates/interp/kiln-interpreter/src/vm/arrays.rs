use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// `Array.from(source, mapFn?)`.
    ///
    /// Anything iterable is walked through the protocol. Anything else is treated as array-like
    /// and read by index up to its `length`, which is the other half of what the specification
    /// says and what makes `Array.from({ length: 2 })` two undefineds rather than an error.
    pub(crate) fn array_from(
        &mut self,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let source = arguments.first().cloned().unwrap_or(Value::Undefined);
        let map = arguments.get(1).cloned().unwrap_or(Value::Undefined);
        let this_argument = arguments.get(2).cloned().unwrap_or(Value::Undefined);
        // The mapper is checked before the source is read, which is the order the specification
        // uses and the reason `Array.from([], null)` is a TypeError rather than a copy: `null`
        // is not "no mapper", only `undefined` is.
        let mapper = match &map {
            Value::Undefined => None,
            value if is_callable(value) => Some(value.clone()),
            value => {
                return Err(ExecutionError::NotCallable {
                    instruction,
                    value: value.type_name(),
                });
            }
        };
        // Neither an iterable nor an array-like: `Array.from` reads `Symbol.iterator` off its
        // argument, and there is nothing to read it off.
        if source.is_nullish() {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{} is not iterable", source.to_display_string()),
            });
        }

        // `Symbol.iterator` is read exactly once and then branched on. Reading it to decide
        // between the two shapes and again to call it would run a user's getter twice -- and
        // every one of these reads is an ordinary `Get`, so an accessor standing in front of it
        // has to run and its throw has to propagate.
        let method =
            self.read_property(&source, WellKnown::Iterator.property_key(), instruction)?;

        if method.is_nullish() {
            // Array-like: read by index up to `length`, which is the other half of what the
            // specification says and what makes `Array.from({ length: 2 })` two undefineds
            // rather than an error.
            //
            // `LengthOfArrayLike` is `ToLength(Get(items, "length"))`, so a length that is not
            // already a number is coerced rather than read as zero -- `Array.from({length: "2"})`
            // is two undefineds -- and one above `2**53-1` is clamped there. Matching on
            // `Value::Number` and casting was wrong in both directions: `{length: "2"}` produced
            // an empty array, and `{length: Infinity}` cast to `usize::MAX` and then *ran*,
            // reaching 6 GB before anything stopped it. The refusal above `u32::MAX` is the
            // `ArrayCreate` step's `RangeError`, which is what makes the infinite case an error a
            // program can catch instead of an allocation the machine cannot survive.
            let length = self.read_property(&source, "length", instruction)?;
            let length = to_length(self.as_number_argument(Some(&length), instruction)?);
            if length > u32::MAX as usize {
                return Err(ExecutionError::InvalidArrayLength);
            }
            let mut collected = Vec::with_capacity(length.min(1024));
            for index in 0..length {
                let value =
                    self.read_computed(&source, &Value::Number(index as f64), instruction)?;
                collected.push(self.array_from_mapped(
                    mapper.as_ref(),
                    &this_argument,
                    value,
                    index,
                    instruction,
                )?);
            }
            return Ok(Value::Array(ArrayData::new(collected)));
        }

        // Already the right shape, and copying beats walking an iterator to the same elements --
        // but only while nothing that runs during the walk could tell the difference. A replaced
        // `Symbol.iterator` is a different sequence, and a mapper is user code that runs between
        // the steps: one that shortens the array as it goes must see the shorter array, because
        // the specification re-reads `length` on every step of the array iterator.
        if let (Value::Array(items), None, Value::Native(Native::ArrayValues, _)) =
            (&source, &mapper, &method)
        {
            return Ok(Value::Array(ArrayData::new(dense_items(&items.borrow()))));
        }

        if !is_callable(&method) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{} is not iterable", source.to_display_string()),
            });
        }
        let iterator = self.call_value(&method, source.clone(), Vec::new(), instruction)?;
        // `GetIteratorFromMethod` reads `next` once and the record keeps it. Reading it again per
        // step is not merely extra work: it is a `Get` the language does not perform, so a proxy
        // counts four traps for a three-element walk and an accessor that answers differently the
        // second time is followed where the specification pins the first answer.
        let next = self.read_property(&iterator, "next", instruction)?;
        let mut collected = Vec::new();
        loop {
            let step = self.call_value(&next, iterator.clone(), Vec::new(), instruction)?;
            // A step that is not an object has no `done` to read, and treating its absence as
            // "not done" is a walk that never ends.
            if !is_an_object(&step) {
                return Err(ExecutionError::IteratorResultNotAnObject);
            }
            if self.read_property(&step, "done", instruction)?.is_truthy() {
                return Ok(Value::Array(ArrayData::new(collected)));
            }
            let value = self.read_property(&step, "value", instruction)?;
            // The mapper runs *inside* the walk rather than over the finished list. It sees each
            // element as the iterator produces it, and a throw from it closes an iterator that is
            // still open. Draining first got both wrong, and left an endless iterable running
            // where the language stops at the first mapper that throws.
            let mapped = self.array_from_mapped(
                mapper.as_ref(),
                &this_argument,
                value,
                collected.len(),
                instruction,
            );
            match mapped {
                Ok(value) => collected.push(value),
                Err(error) => {
                    self.close_discarding(&iterator, instruction);
                    return Err(error);
                }
            }
            if collected.len() > self.limits.max_steps as usize {
                return Err(ExecutionError::StepLimitExceeded {
                    steps: collected.len() as u64,
                    limit: self.limits.max_steps,
                });
            }
        }
    }

    /// One element of `Array.from`, through the mapper if there is one.
    pub(crate) fn array_from_mapped(
        &mut self,
        mapper: Option<&Value>,
        this_argument: &Value,
        value: Value,
        index: usize,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        match mapper {
            None => Ok(value),
            Some(mapper) => self.call_value(
                mapper,
                this_argument.clone(),
                vec![value, Value::Number(index as f64)],
                instruction,
            ),
        }
    }

    pub(crate) fn drain_iterable(
        &mut self,
        value: &Value,
        instruction: usize,
    ) -> Result<Vec<Value>, ExecutionError> {
        let method = self.read_property(value, WellKnown::Iterator.property_key(), instruction)?;
        self.drain_with(value, method, instruction)
    }

    /// Walk an iterable to exhaustion, given the `Symbol.iterator` already read off it.
    ///
    /// Split from `drain_iterable` for the callers that had to read the method themselves to
    /// decide whether to walk at all: `Symbol.iterator` may be an accessor, and asking for it a
    /// second time would run that getter twice.
    pub(crate) fn drain_with(
        &mut self,
        value: &Value,
        method: Value,
        instruction: usize,
    ) -> Result<Vec<Value>, ExecutionError> {
        // Said as "not iterable" rather than as "undefined is not a function": what the program
        // got wrong is the value it destructured or spread, and the missing method is an
        // implementation detail of finding that out.
        if !is_callable(&method) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{} is not iterable", value.to_display_string()),
            });
        }
        let iterator = self.call_value(&method, value.clone(), Vec::new(), instruction)?;
        // Every read here is an ordinary `Get`, so an accessor in front of `next`, `done` or
        // `value` runs and is allowed to throw. Reading them accessor-blind did not merely lose a
        // value: a getter that throws is how a test says "stop", and swallowing it turned the
        // walk into a loop that never ends and accumulates a value per step.
        //
        // `next` is read once and kept, because that is what an iterator record *is*: the
        // specification captures it beside the iterator in `GetIteratorFromMethod` and every step
        // calls what was captured. Re-reading it per step made a three-element spread ask for
        // `next` four times where the language asks once, which a proxy and an accessor both see.
        let next = self.read_property(&iterator, "next", instruction)?;

        let mut collected = Vec::new();
        loop {
            let step = self.call_value(&next, iterator.clone(), Vec::new(), instruction)?;
            // A step that is not an object has no `done` to read, and treating its absence as
            // "not done" is the same non-terminating walk by another route.
            if !is_an_object(&step) {
                return Err(ExecutionError::IteratorResultNotAnObject);
            }
            if self.read_property(&step, "done", instruction)?.is_truthy() {
                return Ok(collected);
            }
            collected.push(self.read_property(&step, "value", instruction)?);
            if collected.len() > self.limits.max_steps as usize {
                return Err(ExecutionError::StepLimitExceeded {
                    steps: collected.len() as u64,
                    limit: self.limits.max_steps,
                });
            }
        }
    }

    /// Run an array method that takes a callback.
    ///
    /// The length is re-read each step, so an array mutated by the callback behaves the way
    /// JavaScript says: elements appended during the walk are not visited, and a shortened array
    /// stops early rather than reading past its end.
    pub(crate) fn array_callback(
        &mut self,
        method: CallbackMethod,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        // The three that build a new array go the same way the four natives do, and for the same
        // reason: the species is decided by reading the receiver's `constructor`, and this walk
        // has no way to consult it that the generic one does not already have.
        if matches!(
            method,
            CallbackMethod::Map
                | CallbackMethod::ParallelMap
                | CallbackMethod::Filter
                | CallbackMethod::FlatMap
        ) {
            return self.generic_array_callback(method, receiver, arguments, instruction);
        }
        let Some(items) = dense_array(receiver) else {
            return self.generic_array_callback(method, receiver, arguments, instruction);
        };
        let callback = arguments.first().cloned().unwrap_or(Value::Undefined);
        let this_value = arguments.get(1).cloned().unwrap_or(Value::Undefined);

        // Checked before the walk rather than at the first call: `[].map(undefined)` is a
        // `TypeError` in the specification and an empty array has no element to discover that
        // with, so an empty receiver would otherwise excuse a callback that is not one.
        if !is_callable(&callback) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{} is not a function", callback.to_display_string()),
            });
        }

        let mut collected = Vec::new();
        let mut index = 0usize;
        loop {
            let Some(item) = items.borrow().get(index).cloned() else {
                break;
            };
            // Every element-walking method skips a hole and visits an undefined. `map` still
            // has to *leave* one: its result has an entry per input position, so a skipped hole
            // that produced nothing would shorten the array and shift everything after it.
            if Self::is_hole(&item) {
                if matches!(method, CallbackMethod::Map | CallbackMethod::ParallelMap) {
                    collected.push(Value::Native(Native::Hole, RealmId::PRIMARY));
                }
                index += 1;
                continue;
            }
            let result = self.call_value(
                &callback,
                this_value.clone(),
                vec![item.clone(), Value::Number(index as f64), receiver.clone()],
                instruction,
            )?;
            match method {
                // Dispatched before `array_callback` is reached; a generator method is not an
                // array walk.
                CallbackMethod::GeneratorNext
                | CallbackMethod::GeneratorReturn
                | CallbackMethod::GeneratorThrow
                | CallbackMethod::AsyncGeneratorNext
                | CallbackMethod::AsyncGeneratorReturn
                | CallbackMethod::AsyncGeneratorThrow => {
                    unreachable!("generator methods are handled by `invoke`")
                }
                CallbackMethod::Reduce => {
                    unreachable!("`reduce` is handled by `array_reduce`")
                }
                CallbackMethod::Sort => unreachable!("`sort` is handled by `array_sort`"),
                CallbackMethod::FindIndex => {
                    if result.is_truthy() {
                        return Ok(Value::Number(index as f64));
                    }
                }
                CallbackMethod::Map | CallbackMethod::ParallelMap => collected.push(result),
                CallbackMethod::ForEach => {}
                CallbackMethod::Filter => {
                    if result.is_truthy() {
                        collected.push(item);
                    }
                }
                CallbackMethod::Find => {
                    if result.is_truthy() {
                        return Ok(item);
                    }
                }
                CallbackMethod::Some => {
                    if result.is_truthy() {
                        return Ok(Value::Boolean(true));
                    }
                }
                CallbackMethod::Every => {
                    if !result.is_truthy() {
                        return Ok(Value::Boolean(false));
                    }
                }
                CallbackMethod::FlatMap => collected.push(result),
                // Walked backwards below rather than here, so reaching this is a dispatch bug.
                CallbackMethod::FindLast | CallbackMethod::FindLastIndex => {
                    unreachable!("the last-first walks are handled by `array_callback_backwards`")
                }
                CallbackMethod::ReduceRight => {
                    unreachable!("`reduceRight` is handled by `array_reduce`")
                }
            }
            index += 1;
        }

        Ok(match method {
            CallbackMethod::GeneratorNext
            | CallbackMethod::GeneratorReturn
            | CallbackMethod::GeneratorThrow
            | CallbackMethod::AsyncGeneratorNext
            | CallbackMethod::AsyncGeneratorReturn
            | CallbackMethod::AsyncGeneratorThrow => {
                unreachable!("generator methods are handled by `invoke`")
            }
            CallbackMethod::Reduce | CallbackMethod::ReduceRight => {
                unreachable!("`reduce` is handled by `array_reduce`")
            }
            CallbackMethod::Sort => unreachable!("`sort` is handled by `array_sort`"),
            CallbackMethod::FindLast | CallbackMethod::FindLastIndex => {
                unreachable!("the last-first walks are handled by `array_callback_backwards`")
            }
            // One level, and only of what the callback produced -- `flatMap` does not flatten an
            // array that was already an element.
            CallbackMethod::FlatMap => Value::Array(ArrayData::new(flatten(&collected, 1))),
            // Not undefined: `findIndex` reports -1 for "nowhere", where `find` reports the
            // absence of a value.
            CallbackMethod::FindIndex => Value::Number(-1.0),
            CallbackMethod::Map | CallbackMethod::ParallelMap | CallbackMethod::Filter => {
                Value::Array(ArrayData::new(collected))
            }
            CallbackMethod::ForEach | CallbackMethod::Find => Value::Undefined,
            CallbackMethod::Some => Value::Boolean(false),
            CallbackMethod::Every => Value::Boolean(true),
        })
    }

    /// `reduce(callback, initial?)`.
    ///
    /// With no initial value the first element seeds the accumulator and the walk starts at the
    /// second, which is why this cannot share `array_callback`'s loop: the starting index depends
    /// on an argument, and the callback takes four parameters rather than three.
    /// `findLast` and `findLastIndex`, which walk from the end.
    ///
    /// A separate walk rather than a flag on the forward one: the length is re-read each step so a
    /// callback that shortens the array stops the walk, and "the step after the last one" is a
    /// different thing when counting down.
    pub(crate) fn array_callback_backwards(
        &mut self,
        method: CallbackMethod,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Some(items) = dense_array(receiver) else {
            return self.generic_array_callback(method, receiver, arguments, instruction);
        };
        let callback = arguments.first().cloned().unwrap_or(Value::Undefined);
        let this_value = arguments.get(1).cloned().unwrap_or(Value::Undefined);

        // Checked before the walk rather than at the first call: `[].map(undefined)` is a
        // `TypeError` in the specification and an empty array has no element to discover that
        // with, so an empty receiver would otherwise excuse a callback that is not one.
        if !is_callable(&callback) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{} is not a function", callback.to_display_string()),
            });
        }
        let mut index = items.borrow().len();
        while index > 0 {
            index -= 1;
            // A hole is visited as undefined here, unlike `find`: the last-first pair were added
            // to the language after the decision to stop skipping them.
            let item = items
                .borrow()
                .get(index)
                .cloned()
                .filter(|item| !Self::is_hole(item))
                .unwrap_or(Value::Undefined);
            let result = self.call_value(
                &callback,
                this_value.clone(),
                vec![item.clone(), Value::Number(index as f64), receiver.clone()],
                instruction,
            )?;
            if result.is_truthy() {
                return Ok(if matches!(method, CallbackMethod::FindLastIndex) {
                    Value::Number(index as f64)
                } else {
                    item
                });
            }
        }
        Ok(if matches!(method, CallbackMethod::FindLastIndex) {
            Value::Number(-1.0)
        } else {
            Value::Undefined
        })
    }

    pub(crate) fn array_reduce(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        backwards: bool,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Some(items) = dense_array(receiver) else {
            let method = if backwards {
                CallbackMethod::ReduceRight
            } else {
                CallbackMethod::Reduce
            };
            return self.generic_array_callback(method, receiver, arguments, instruction);
        };
        let callback = arguments.first().cloned().unwrap_or(Value::Undefined);

        // Checked before the walk rather than at the first call: `[].map(undefined)` is a
        // `TypeError` in the specification and an empty array has no element to discover that
        // with, so an empty receiver would otherwise excuse a callback that is not one.
        if !is_callable(&callback) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{} is not a function", callback.to_display_string()),
            });
        }
        let length = items.borrow().len();
        let (mut accumulator, mut index) = match arguments.get(1) {
            Some(initial) => (
                initial.clone(),
                if backwards { length.wrapping_sub(1) } else { 0 },
            ),
            None => {
                // Reducing an empty array with no initial value has no answer to give, which is
                // a catchable TypeError in the specification and catchable here too.
                // The seed is the first element that is *there* from whichever end the walk
                // starts: an array of nothing but holes has no answer to give, exactly as an
                // empty one does not.
                let borrowed = items.borrow();
                let found = if backwards {
                    borrowed.iter().rposition(|item| !Self::is_hole(item))
                } else {
                    borrowed.iter().position(|item| !Self::is_hole(item))
                };
                let Some(at) = found else {
                    return Err(ExecutionError::EmptyReduce);
                };
                (
                    borrowed[at].clone(),
                    if backwards {
                        at.wrapping_sub(1)
                    } else {
                        at + 1
                    },
                )
            }
        };
        loop {
            // The length is re-read each step, so a callback that shortens the array stops the
            // walk rather than reading past the end -- the same rule `array_callback` follows.
            // Walking backwards, the step past the start wraps, which `get` reports the same way.
            let Some(item) = items.borrow().get(index).cloned() else {
                return Ok(accumulator);
            };
            // `reduce` walks elements, and a hole is not one: visiting it would fold undefined
            // into the accumulator and turn every sum over a sparse array into NaN.
            if Self::is_hole(&item) {
                index = if backwards {
                    index.wrapping_sub(1)
                } else {
                    index + 1
                };
                continue;
            }
            accumulator = self.call_value(
                &callback,
                Value::Undefined,
                vec![
                    accumulator,
                    item,
                    Value::Number(index as f64),
                    receiver.clone(),
                ],
                instruction,
            )?;
            index = if backwards {
                index.wrapping_sub(1)
            } else {
                index + 1
            };
        }
    }

    /// `sort(comparator?)`, in place.
    ///
    /// A bottom-up merge sort rather than `slice::sort_by`, for two reasons: the comparator can
    /// be user code, so each comparison has to be able to re-enter the interpreter, and the sort
    /// has to be stable, which the language requires and merging gives for free.
    ///
    /// Undefined elements sort to the end without ever reaching the comparator, and the default
    /// comparison is by string -- which is why `[10, 9, 1].sort()` is `[1, 10, 9]`.
    pub(crate) fn array_sort(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Some(items) = dense_array(receiver) else {
            return self.generic_array_sort(receiver, arguments, instruction);
        };
        let comparator = arguments.first().cloned().unwrap_or(Value::Undefined);
        // `undefined` means the default ordering; anything else has to be callable, and the
        // check runs before the array is touched so a refused sort leaves it as it was.
        if !matches!(comparator, Value::Undefined) && !is_callable(&comparator) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "The comparison function must be either a function or undefined"
                    .to_owned(),
            });
        }
        let taken = std::mem::take(&mut *items.borrow_mut());
        let (mut present, absent): (Vec<_>, Vec<_>) = taken
            .into_iter()
            .partition(|item| !matches!(item, Value::Undefined));

        self.timsort(&comparator, &mut present, instruction)?;

        present.extend(absent);
        *items.borrow_mut() = present;
        // In place, and yields the same array rather than a copy.
        Ok(receiver.clone())
    }

    pub(crate) fn timsort_min_run_length(mut n: usize) -> usize {
        let mut r = 0;
        while n >= 64 {
            r |= n & 1;
            n >>= 1;
        }
        n + r
    }

    pub(crate) fn timsort(
        &mut self,
        comparator: &Value,
        arr: &mut [Value],
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let len = arr.len();
        if len < 2 {
            return Ok(());
        }
        if len < 32 {
            let run_len = self.timsort_count_run(comparator, arr, instruction)?;
            return self.timsort_binary_sort(comparator, arr, run_len, instruction);
        }
        let min_run = Self::timsort_min_run_length(len);
        let mut runs: Vec<(usize, usize)> = Vec::with_capacity(64);
        let mut lo = 0;
        while lo < len {
            let mut run_len = self.timsort_count_run(comparator, &mut arr[lo..], instruction)?;
            if run_len < min_run {
                let force = (len - lo).min(min_run);
                self.timsort_binary_sort(
                    comparator,
                    &mut arr[lo..lo + force],
                    run_len,
                    instruction,
                )?;
                run_len = force;
            }
            runs.push((lo, run_len));
            self.timsort_merge_collapse(comparator, arr, &mut runs, instruction)?;
            lo += run_len;
        }
        self.timsort_merge_force_collapse(comparator, arr, &mut runs, instruction)?;
        Ok(())
    }

    pub(crate) fn timsort_binary_sort(
        &mut self,
        comparator: &Value,
        slice: &mut [Value],
        start: usize,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let len = slice.len();
        let start = start.max(1);
        for i in start..len {
            let pivot = slice[i].clone();
            let mut left = 0;
            let mut right = i;
            while left < right {
                let mid = left + ((right - left) >> 1);
                let order = self.compare_for_sort(comparator, &pivot, &slice[mid], instruction)?;
                if order < 0.0 {
                    right = mid;
                } else {
                    left = mid + 1;
                }
            }
            slice[left..=i].rotate_right(1);
        }
        Ok(())
    }

    pub(crate) fn timsort_count_run(
        &mut self,
        comparator: &Value,
        slice: &mut [Value],
        instruction: usize,
    ) -> Result<usize, ExecutionError> {
        let len = slice.len();
        if len <= 1 {
            return Ok(len);
        }
        let order = self.compare_for_sort(comparator, &slice[1], &slice[0], instruction)?;
        let mut run_hi = 2;
        if order < 0.0 {
            while run_hi < len {
                let ord = self.compare_for_sort(
                    comparator,
                    &slice[run_hi],
                    &slice[run_hi - 1],
                    instruction,
                )?;
                if ord < 0.0 {
                    run_hi += 1;
                } else {
                    break;
                }
            }
            slice[..run_hi].reverse();
        } else {
            while run_hi < len {
                let ord = self.compare_for_sort(
                    comparator,
                    &slice[run_hi],
                    &slice[run_hi - 1],
                    instruction,
                )?;
                if ord >= 0.0 {
                    run_hi += 1;
                } else {
                    break;
                }
            }
        }
        Ok(run_hi)
    }

    pub(crate) fn timsort_merge_collapse(
        &mut self,
        comparator: &Value,
        arr: &mut [Value],
        runs: &mut Vec<(usize, usize)>,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        while runs.len() > 1 {
            let mut n = runs.len() - 2;
            if n > 0 && runs[n - 1].1 <= runs[n].1 + runs[n + 1].1 {
                if runs[n - 1].1 < runs[n + 1].1 {
                    n -= 1;
                }
                self.timsort_merge_at(comparator, arr, runs, n, instruction)?;
            } else if runs[n].1 <= runs[n + 1].1 {
                self.timsort_merge_at(comparator, arr, runs, n, instruction)?;
            } else {
                break;
            }
        }
        Ok(())
    }

    pub(crate) fn timsort_merge_force_collapse(
        &mut self,
        comparator: &Value,
        arr: &mut [Value],
        runs: &mut Vec<(usize, usize)>,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        while runs.len() > 1 {
            let mut n = runs.len() - 2;
            if n > 0 && runs[n - 1].1 < runs[n + 1].1 {
                n -= 1;
            }
            self.timsort_merge_at(comparator, arr, runs, n, instruction)?;
        }
        Ok(())
    }

    pub(crate) fn timsort_merge_at(
        &mut self,
        comparator: &Value,
        arr: &mut [Value],
        runs: &mut Vec<(usize, usize)>,
        i: usize,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let (base1, len1) = runs[i];
        let (base2, len2) = runs[i + 1];
        runs[i] = (base1, len1 + len2);
        runs.remove(i + 1);

        let mut k = 0;
        while k < len1 {
            let ord =
                self.compare_for_sort(comparator, &arr[base2], &arr[base1 + k], instruction)?;
            if ord < 0.0 {
                break;
            }
            k += 1;
        }
        let (base1, len1) = (base1 + k, len1 - k);
        if len1 == 0 {
            return Ok(());
        }

        let mut m = len2;
        while m > 0 {
            let ord = self.compare_for_sort(
                comparator,
                &arr[base1 + len1 - 1],
                &arr[base2 + m - 1],
                instruction,
            )?;
            if ord >= 0.0 {
                break;
            }
            m -= 1;
        }
        let len2 = m;
        if len2 == 0 {
            return Ok(());
        }

        if len1 <= len2 {
            let scratch = arr[base1..base1 + len1].to_vec();
            let (mut c1, mut c2, mut dest) = (0, base2, base1);
            while c1 < len1 && c2 < base2 + len2 {
                let ord = self.compare_for_sort(comparator, &arr[c2], &scratch[c1], instruction)?;
                if ord < 0.0 {
                    arr[dest] = arr[c2].clone();
                    c2 += 1;
                } else {
                    arr[dest] = scratch[c1].clone();
                    c1 += 1;
                }
                dest += 1;
            }
            while c1 < len1 {
                arr[dest] = scratch[c1].clone();
                dest += 1;
                c1 += 1;
            }
        } else {
            let scratch = arr[base2..base2 + len2].to_vec();
            let (mut c1, mut c2, mut dest) = (base1 + len1, len2, base2 + len2);
            while c1 > base1 && c2 > 0 {
                let ord =
                    self.compare_for_sort(comparator, &scratch[c2 - 1], &arr[c1 - 1], instruction)?;
                if ord < 0.0 {
                    arr[dest - 1] = arr[c1 - 1].clone();
                    c1 -= 1;
                } else {
                    arr[dest - 1] = scratch[c2 - 1].clone();
                    c2 -= 1;
                }
                dest -= 1;
            }
            while c2 > 0 {
                arr[dest - 1] = scratch[c2 - 1].clone();
                dest -= 1;
                c2 -= 1;
            }
        }
        Ok(())
    }

    /// One comparison for `array_sort`: the comparator's number, or the default string order.
    pub(crate) fn compare_for_sort(
        &mut self,
        comparator: &Value,
        left: &Value,
        right: &Value,
        instruction: usize,
    ) -> Result<f64, ExecutionError> {
        if comparator.is_nullish() {
            let (left, right) = (
                left.to_string_units_checked()?,
                right.to_string_units_checked()?,
            );
            // UTF-16 code units, which is the order the language compares strings in.
            return Ok(match left.cmp(&right) {
                std::cmp::Ordering::Less => -1.0,
                std::cmp::Ordering::Equal => 0.0,
                std::cmp::Ordering::Greater => 1.0,
            });
        }
        let result = self.call_value(
            comparator,
            Value::Undefined,
            vec![left.clone(), right.clone()],
            instruction,
        )?;
        let ordering = result.as_number();
        // A comparator returning NaN is treated as "equal", which keeps the merge total rather
        // than letting an inconsistent comparator corrupt the result.
        Ok(if ordering.is_nan() { 0.0 } else { ordering })
    }
}
