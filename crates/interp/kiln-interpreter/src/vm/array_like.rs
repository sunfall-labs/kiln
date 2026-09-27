use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    pub(crate) fn generic_array_native(
        &mut self,
        native: Native,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let receiver = &self.as_receiver_object(receiver.clone())?;
        let mut steps = 0;
        match native {
            Native::ArrayAt => {
                let length = self.array_like_length(receiver, instruction)?;
                let relative = Self::to_integer_or_infinity(
                    self.as_number_argument(arguments.first(), instruction)?,
                );
                #[expect(clippy::cast_precision_loss)]
                let span = length as f64;
                let index = if relative >= 0.0 {
                    relative
                } else {
                    span + relative
                };
                if index < 0.0 || index >= span {
                    return Ok(Value::Undefined);
                }
                #[expect(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                let index = index as u64;
                self.element_get(receiver, index, instruction)
            }
            Native::ArrayPush => {
                let length = self.array_like_length(receiver, instruction)?;
                let count = arguments.len() as u64;
                Self::bounded_index(length, count)?;
                for (offset, argument) in arguments.iter().enumerate() {
                    self.element_set(
                        receiver,
                        length + offset as u64,
                        argument.clone(),
                        instruction,
                    )?;
                }
                let grown = length + count;
                self.set_array_like_length(receiver, grown, instruction)?;
                #[expect(clippy::cast_precision_loss)]
                let grown = grown as f64;
                Ok(Value::Number(grown))
            }
            Native::ArrayPop => {
                let length = self.array_like_length(receiver, instruction)?;
                if length == 0 {
                    self.set_array_like_length(receiver, 0, instruction)?;
                    return Ok(Value::Undefined);
                }
                let last = length - 1;
                let value = self.element_get(receiver, last, instruction)?;
                self.element_delete(receiver, last, instruction)?;
                self.set_array_like_length(receiver, last, instruction)?;
                Ok(value)
            }
            Native::ArrayShift => {
                let length = self.array_like_length(receiver, instruction)?;
                if length == 0 {
                    self.set_array_like_length(receiver, 0, instruction)?;
                    return Ok(Value::Undefined);
                }
                let first = self.element_get(receiver, 0, instruction)?;
                for index in 1..length {
                    Self::walk_step(&mut steps)?;
                    self.move_element(receiver, index, index - 1, instruction)?;
                }
                self.element_delete(receiver, length - 1, instruction)?;
                self.set_array_like_length(receiver, length - 1, instruction)?;
                Ok(first)
            }
            Native::ArrayUnshift => {
                let length = self.array_like_length(receiver, instruction)?;
                let count = arguments.len() as u64;
                if count > 0 {
                    Self::bounded_index(length, count)?;
                    let mut index = length;
                    while index > 0 {
                        Self::walk_step(&mut steps)?;
                        self.move_element(receiver, index - 1, index + count - 1, instruction)?;
                        index -= 1;
                    }
                    for (offset, argument) in arguments.iter().enumerate() {
                        self.element_set(receiver, offset as u64, argument.clone(), instruction)?;
                    }
                }
                let grown = length + count;
                self.set_array_like_length(receiver, grown, instruction)?;
                #[expect(clippy::cast_precision_loss)]
                let grown = grown as f64;
                Ok(Value::Number(grown))
            }
            Native::ArrayReverse => {
                let length = self.array_like_length(receiver, instruction)?;
                let middle = length / 2;
                let mut lower = 0;
                while lower != middle {
                    Self::walk_step(&mut steps)?;
                    let upper = length - lower - 1;
                    let lower_there = self.element_has(receiver, lower, instruction)?;
                    let lower_value = if lower_there {
                        self.element_get(receiver, lower, instruction)?
                    } else {
                        Value::Undefined
                    };
                    let upper_there = self.element_has(receiver, upper, instruction)?;
                    let upper_value = if upper_there {
                        self.element_get(receiver, upper, instruction)?
                    } else {
                        Value::Undefined
                    };
                    // A hole swapped with a value has to stay a hole on the other side, which
                    // means deleting rather than writing an undefined: the two are the same
                    // read and a different answer to `in`.
                    if upper_there {
                        self.element_set(receiver, lower, upper_value, instruction)?;
                    } else {
                        self.element_delete(receiver, lower, instruction)?;
                    }
                    if lower_there {
                        self.element_set(receiver, upper, lower_value, instruction)?;
                    } else {
                        self.element_delete(receiver, upper, instruction)?;
                    }
                    lower += 1;
                }
                Ok(receiver.clone())
            }
            Native::ArrayFill => {
                let length = self.array_like_length(receiver, instruction)?;
                let value = arguments.first().cloned().unwrap_or(Value::Undefined);
                let start = self.bound_argument(arguments.get(1), length, 0, instruction)?;
                let end = self.bound_argument(arguments.get(2), length, length, instruction)?;
                let mut index = start;
                while index < end {
                    Self::walk_step(&mut steps)?;
                    self.element_set(receiver, index, value.clone(), instruction)?;
                    index += 1;
                }
                Ok(receiver.clone())
            }
            Native::ArrayCopyWithin => {
                let length = self.array_like_length(receiver, instruction)?;
                let to = self.bound_argument(arguments.first(), length, 0, instruction)?;
                let from = self.bound_argument(arguments.get(1), length, 0, instruction)?;
                let end = self.bound_argument(arguments.get(2), length, length, instruction)?;
                let mut count = end.saturating_sub(from).min(length - to);
                // The source and the destination overlap by design -- that overlap is the whole
                // reason the method exists -- so an overlapping forward copy walks backwards
                // instead, which is what keeps it from reading what it has already written.
                let (mut from, mut to, direction) = if from < to && to < from + count {
                    ((from + count - 1) as i64, (to + count - 1) as i64, -1i64)
                } else {
                    (from as i64, to as i64, 1i64)
                };
                while count > 0 {
                    Self::walk_step(&mut steps)?;
                    self.move_element(receiver, from as u64, to as u64, instruction)?;
                    from += direction;
                    to += direction;
                    count -= 1;
                }
                Ok(receiver.clone())
            }
            Native::ArraySlice => {
                let length = self.array_like_length(receiver, instruction)?;
                let start = self.bound_argument(arguments.first(), length, 0, instruction)?;
                let end = self.bound_argument(arguments.get(1), length, length, instruction)?;
                let count = end.saturating_sub(start);
                let created = self.array_species_create(receiver, count, instruction)?;
                let mut written = 0;
                let mut index = start;
                while index < end {
                    Self::walk_step(&mut steps)?;
                    if let Some(value) = self.element_read(receiver, index, instruction)? {
                        self.create_element(&created, written, value, instruction)?;
                    }
                    index += 1;
                    written += 1;
                }
                self.finish_created_array(&created, written)?;
                Ok(created)
            }
            Native::ArraySplice => {
                let length = self.array_like_length(receiver, instruction)?;
                let start = self.bound_argument(arguments.first(), length, 0, instruction)?;
                let inserted = arguments.len().saturating_sub(2) as u64;
                // A missing count removes everything from `start`, where an explicit `undefined`
                // removes nothing: `splice(1)` and `splice(1, undefined)` are different calls.
                let removed_count = match arguments.len() {
                    0 => 0,
                    1 => length - start,
                    _ => {
                        let wanted = Self::to_integer_or_infinity(
                            self.as_number_argument(arguments.get(1), instruction)?,
                        );
                        if wanted <= 0.0 {
                            0
                        } else {
                            #[expect(clippy::cast_precision_loss)]
                            let room = (length - start) as f64;
                            #[expect(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                            let taken = wanted.min(room) as u64;
                            taken
                        }
                    }
                };
                Self::bounded_index(length + inserted, 0)?;
                let removed = self.array_species_create(receiver, removed_count, instruction)?;
                for offset in 0..removed_count {
                    Self::walk_step(&mut steps)?;
                    if self.element_has(receiver, start + offset, instruction)? {
                        let value = self.element_get(receiver, start + offset, instruction)?;
                        self.create_element(&removed, offset, value, instruction)?;
                    }
                }
                self.finish_created_array(&removed, removed_count)?;
                if inserted < removed_count {
                    let mut index = start;
                    while index < length - removed_count {
                        Self::walk_step(&mut steps)?;
                        self.move_element(
                            receiver,
                            index + removed_count,
                            index + inserted,
                            instruction,
                        )?;
                        index += 1;
                    }
                    let mut index = length;
                    while index > length - removed_count + inserted {
                        Self::walk_step(&mut steps)?;
                        self.element_delete(receiver, index - 1, instruction)?;
                        index -= 1;
                    }
                } else if inserted > removed_count {
                    let mut index = length - removed_count;
                    while index > start {
                        Self::walk_step(&mut steps)?;
                        self.move_element(
                            receiver,
                            index + removed_count - 1,
                            index + inserted - 1,
                            instruction,
                        )?;
                        index -= 1;
                    }
                }
                for (offset, argument) in arguments.iter().skip(2).enumerate() {
                    self.element_set(
                        receiver,
                        start + offset as u64,
                        argument.clone(),
                        instruction,
                    )?;
                }
                self.set_array_like_length(
                    receiver,
                    length - removed_count + inserted,
                    instruction,
                )?;
                Ok(removed)
            }
            Native::ArrayConcat => {
                let created = self.array_species_create(receiver, 0, instruction)?;
                let mut written = 0;
                let mut sources = vec![receiver.clone()];
                sources.extend(arguments.iter().cloned());
                for source in sources {
                    if self.is_concat_spreadable(&source, instruction)? {
                        let length = self.array_like_length(&source, instruction)?;
                        Self::bounded_index(written, length)?;
                        for index in 0..length {
                            Self::walk_step(&mut steps)?;
                            if let Some(value) = self.element_read(&source, index, instruction)? {
                                self.create_element(&created, written, value, instruction)?;
                            }
                            written += 1;
                        }
                    } else {
                        Self::walk_step(&mut steps)?;
                        self.create_element(&created, written, source, instruction)?;
                        written += 1;
                    }
                }
                self.finish_created_array(&created, written)?;
                Ok(created)
            }
            Native::ArrayJoin => {
                let length = self.array_like_length(receiver, instruction)?;
                let separator = match arguments.first() {
                    None | Some(Value::Undefined) => ",".to_owned(),
                    Some(value) => self.as_text(value, instruction)?,
                };
                let mut out = String::new();
                for index in 0..length {
                    Self::walk_step(&mut steps)?;
                    if index > 0 {
                        push_bounded(&mut out, &separator)?;
                    }
                    let element = self.element_get(receiver, index, instruction)?;
                    if !matches!(element, Value::Undefined | Value::Null) {
                        let text = self.as_text(&element, instruction)?;
                        push_bounded(&mut out, &text)?;
                    }
                }
                Ok(Value::String(out.into()))
            }
            // `Array.prototype.toString` is defined as *calling* the receiver's own `join`, and
            // falling back to `Object.prototype.toString` when that is not callable. Joining
            // directly would make `Array.prototype.toString.call({})` the empty string where
            // every engine answers `[object Object]`.
            Native::ArrayJoinDefault => {
                let join = self.read_property(receiver, "join", instruction)?;
                if !is_callable(&join) {
                    return self.dispatch_native(
                        Native::ObjectToStringTag,
                        receiver,
                        &[],
                        instruction,
                    );
                }
                self.call_value(&join, receiver.clone(), Vec::new(), instruction)
            }
            Native::ArrayIndexOf => {
                let length = self.array_like_length(receiver, instruction)?;
                if length == 0 {
                    return Ok(Value::Number(-1.0));
                }
                let needle = arguments.first().cloned().unwrap_or(Value::Undefined);
                let from = Self::to_integer_or_infinity(
                    self.as_number_argument(arguments.get(1), instruction)?,
                );
                #[expect(clippy::cast_precision_loss)]
                let span = length as f64;
                if from >= span {
                    return Ok(Value::Number(-1.0));
                }
                let mut index = Self::relative_bound(from, length);
                while index < length {
                    Self::walk_step(&mut steps)?;
                    if let Some(value) = self.element_read(receiver, index, instruction)?
                        && strict_equal(&value, &needle)
                    {
                        #[expect(clippy::cast_precision_loss)]
                        let found = index as f64;
                        return Ok(Value::Number(found));
                    }
                    index += 1;
                }
                Ok(Value::Number(-1.0))
            }
            Native::ArrayLastIndexOf => {
                let length = self.array_like_length(receiver, instruction)?;
                if length == 0 {
                    return Ok(Value::Number(-1.0));
                }
                let needle = arguments.first().cloned().unwrap_or(Value::Undefined);
                #[expect(clippy::cast_precision_loss)]
                let last = (length - 1) as f64;
                let from = if arguments.len() > 1 {
                    Self::to_integer_or_infinity(
                        self.as_number_argument(arguments.get(1), instruction)?,
                    )
                } else {
                    last
                };
                #[expect(clippy::cast_precision_loss)]
                let span = length as f64;
                let start = if from >= 0.0 {
                    from.min(last)
                } else {
                    span + from
                };
                if start < 0.0 {
                    return Ok(Value::Number(-1.0));
                }
                #[expect(clippy::cast_possible_truncation)]
                let mut index = start as i64;
                while index >= 0 {
                    Self::walk_step(&mut steps)?;
                    let at = index as u64;
                    if let Some(value) = self.element_read(receiver, at, instruction)?
                        && strict_equal(&value, &needle)
                    {
                        #[expect(clippy::cast_precision_loss)]
                        let found = at as f64;
                        return Ok(Value::Number(found));
                    }
                    index -= 1;
                }
                Ok(Value::Number(-1.0))
            }
            // `includes` reads every position rather than asking whether it is there: a hole is
            // an `undefined` to it, which is the one place it differs from `indexOf`.
            Native::ArrayIncludes => {
                let length = self.array_like_length(receiver, instruction)?;
                if length == 0 {
                    return Ok(Value::Boolean(false));
                }
                let needle = arguments.first().cloned().unwrap_or(Value::Undefined);
                let from = Self::to_integer_or_infinity(
                    self.as_number_argument(arguments.get(1), instruction)?,
                );
                #[expect(clippy::cast_precision_loss)]
                let span = length as f64;
                if from >= span {
                    return Ok(Value::Boolean(false));
                }
                let mut index = Self::relative_bound(from, length);
                while index < length {
                    Self::walk_step(&mut steps)?;
                    let value = self.element_get(receiver, index, instruction)?;
                    if same_value_zero(&value, &needle) {
                        return Ok(Value::Boolean(true));
                    }
                    index += 1;
                }
                Ok(Value::Boolean(false))
            }
            Native::ArrayFlat => {
                let length = self.array_like_length(receiver, instruction)?;
                let depth = match arguments.first() {
                    None | Some(Value::Undefined) => 1.0,
                    Some(_) => Self::to_integer_or_infinity(
                        self.as_number_argument(arguments.first(), instruction)?,
                    ),
                };
                let created = self.array_species_create(receiver, 0, instruction)?;
                let mut written = 0;
                self.flatten_into(
                    &created,
                    receiver,
                    length,
                    &mut written,
                    depth,
                    &mut steps,
                    instruction,
                )?;
                self.finish_created_array(&created, written)?;
                Ok(created)
            }
            // `join` with `ToString` replaced by a *call* to each element's own
            // `toLocaleString`, and the arguments passed straight through: the locale and the
            // options a program hands the array are the ones each element is asked with.
            Native::ArrayToLocaleString => {
                let length = self.array_like_length(receiver, instruction)?;
                let mut out = String::new();
                for index in 0..length {
                    Self::walk_step(&mut steps)?;
                    if index > 0 {
                        push_bounded(&mut out, ",")?;
                    }
                    let element = self.element_get(receiver, index, instruction)?;
                    // `undefined` and `null` render as nothing rather than as themselves, which
                    // is the one rule this shares with `join`.
                    if matches!(element, Value::Undefined | Value::Null) {
                        continue;
                    }
                    let method = self.read_property(&element, "toLocaleString", instruction)?;
                    if !is_callable(&method) {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: "toLocaleString is not a function".to_owned(),
                        });
                    }
                    let rendered =
                        self.call_value(&method, element, arguments.to_vec(), instruction)?;
                    let text = self.as_text(&rendered, instruction)?;
                    push_bounded(&mut out, &text)?;
                }
                Ok(Value::String(out.into()))
            }
            // The three iterators keep the receiver rather than a copy of it, which is the whole
            // point of an iterator: a write between `entries()` and the `next()` that reaches it
            // has to be visible, and a snapshot is a promise that it will not be.
            Native::ArrayEntries | Native::ArrayKeys | Native::ArrayValues => {
                Ok(array_iterator_over(native, receiver))
            }
            Native::ArrayCopy(method) => {
                self.generic_array_copy(method, receiver, arguments, instruction)
            }
            // Everything reaching here is in `walks_a_live_receiver`, which is the list the
            // arms above answer to.
            _ => Ok(Value::Undefined),
        }
    }

    /// `2 ** 53 - 1` is the largest index the language will name, and a method that would push
    /// past it refuses rather than wrapping around to a smaller array than it started with.
    pub(crate) fn bounded_index(length: u64, added: u64) -> Result<(), ExecutionError> {
        #[expect(clippy::cast_precision_loss)]
        let total = length as f64 + added as f64;
        if total > MAX_SAFE_INTEGER {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Invalid array length".to_owned(),
            });
        }
        Ok(())
    }

    /// A `start`/`end` argument, converted through the program's own `valueOf` and resolved
    /// against the length. An argument that is absent or `undefined` means the default.
    pub(crate) fn bound_argument(
        &mut self,
        argument: Option<&Value>,
        length: u64,
        default: u64,
        instruction: usize,
    ) -> Result<u64, ExecutionError> {
        match argument {
            None | Some(Value::Undefined) => Ok(default),
            Some(_) => {
                let relative =
                    Self::to_integer_or_infinity(self.as_number_argument(argument, instruction)?);
                Ok(Self::relative_bound(relative, length))
            }
        }
    }

    /// The `if HasProperty(O, from) then Set(O, to, Get(O, from)) else DeletePropertyOrThrow(O,
    /// to)` step that `shift`, `unshift`, `splice` and `copyWithin` are all built out of.
    ///
    /// Written once because getting it wrong the same way four times is what a copied loop is
    /// for: a hole moved as an `undefined` is a different array afterwards.
    pub(crate) fn move_element(
        &mut self,
        target: &Value,
        from: u64,
        to: u64,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        if self.element_has(target, from, instruction)? {
            let value = self.element_get(target, from, instruction)?;
            self.element_set(target, to, value, instruction)
        } else {
            self.element_delete(target, to, instruction)
        }
    }

    /// `FlattenIntoArray`, one level at a time.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn flatten_into(
        &mut self,
        into: &Value,
        source: &Value,
        length: u64,
        written: &mut u64,
        depth: f64,
        steps: &mut u64,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        for index in 0..length {
            Self::walk_step(steps)?;
            let Some(value) = self.element_read(source, index, instruction)? else {
                continue;
            };
            if depth > 0.0 && matches!(unwrap_proxy(&value), Value::Array(_)) {
                let inner = self.array_like_length(&value, instruction)?;
                self.flatten_into(
                    into,
                    &value,
                    inner,
                    written,
                    depth - 1.0,
                    steps,
                    instruction,
                )?;
            } else {
                self.create_element(into, *written, value, instruction)?;
                *written += 1;
            }
        }
        Ok(())
    }

    /// The four copying methods -- `with`, `toReversed`, `toSorted`, `toSpliced` -- over a
    /// receiver that is not an array. Each answers a plain array of the length it counted, with
    /// every position read: these are the methods that deliberately have no holes.
    pub(crate) fn generic_array_copy(
        &mut self,
        method: ArrayCopy,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let length = self.array_like_length(receiver, instruction)?;
        let mut steps = 0;
        match method {
            ArrayCopy::With => {
                let relative = Self::to_integer_or_infinity(
                    self.as_number_argument(arguments.first(), instruction)?,
                );
                #[expect(clippy::cast_precision_loss)]
                let span = length as f64;
                let index = if relative >= 0.0 {
                    relative
                } else {
                    span + relative
                };
                if index < 0.0 || index >= span {
                    return Err(ExecutionError::ArrayIndexOutOfRange);
                }
                #[expect(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                let index = index as u64;
                let created = Self::create_array_of_length(length)?;
                let replacement = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                for at in 0..length {
                    Self::walk_step(&mut steps)?;
                    let value = if at == index {
                        replacement.clone()
                    } else {
                        self.element_get(receiver, at, instruction)?
                    };
                    self.create_element(&created, at, value, instruction)?;
                }
                Ok(created)
            }
            ArrayCopy::ToReversed => {
                let created = Self::create_array_of_length(length)?;
                for at in 0..length {
                    Self::walk_step(&mut steps)?;
                    let value = self.element_get(receiver, length - at - 1, instruction)?;
                    self.create_element(&created, at, value, instruction)?;
                }
                Ok(created)
            }
            ArrayCopy::ToSorted => {
                let comparator = arguments.first().cloned().unwrap_or(Value::Undefined);
                if !matches!(comparator, Value::Undefined) && !is_callable(&comparator) {
                    return Err(comparator_refused());
                }
                let created = Self::create_array_of_length(length)?;
                for at in 0..length {
                    Self::walk_step(&mut steps)?;
                    let value = self.element_get(receiver, at, instruction)?;
                    self.create_element(&created, at, value, instruction)?;
                }
                self.array_sort(&created, arguments, instruction)
            }
            ArrayCopy::ToSpliced => {
                let start = self.bound_argument(arguments.first(), length, 0, instruction)?;
                let removed = match arguments.len() {
                    0 => 0,
                    1 => length - start,
                    _ => {
                        let wanted = Self::to_integer_or_infinity(
                            self.as_number_argument(arguments.get(1), instruction)?,
                        );
                        if wanted <= 0.0 {
                            0
                        } else {
                            #[expect(clippy::cast_precision_loss)]
                            let room = (length - start) as f64;
                            #[expect(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                            let taken = wanted.min(room) as u64;
                            taken
                        }
                    }
                };
                let inserted = arguments.len().saturating_sub(2) as u64;
                let total = length - removed + inserted;
                let created = Self::create_array_of_length(total)?;
                let mut written = 0;
                for at in 0..start {
                    Self::walk_step(&mut steps)?;
                    let value = self.element_get(receiver, at, instruction)?;
                    self.create_element(&created, written, value, instruction)?;
                    written += 1;
                }
                for argument in arguments.iter().skip(2) {
                    self.create_element(&created, written, argument.clone(), instruction)?;
                    written += 1;
                }
                let mut at = start + removed;
                while at < length {
                    Self::walk_step(&mut steps)?;
                    let value = self.element_get(receiver, at, instruction)?;
                    self.create_element(&created, written, value, instruction)?;
                    written += 1;
                    at += 1;
                }
                Self::truncate_created_array(&created, written);
                Ok(created)
            }
        }
    }

    /// One of the callback-taking `Array.prototype` methods over a receiver that is not an
    /// array.
    pub(crate) fn generic_array_callback(
        &mut self,
        method: CallbackMethod,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let receiver = &self.as_receiver_object(receiver.clone())?;
        if matches!(method, CallbackMethod::Sort) {
            return self.generic_array_sort(receiver, arguments, instruction);
        }
        let length = self.array_like_length(receiver, instruction)?;
        let callback = arguments.first().cloned().unwrap_or(Value::Undefined);
        // Checked before the walk rather than at the first call: an empty receiver has no
        // element to discover it with, and would otherwise excuse a callback that is not one.
        if !is_callable(&callback) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("{} is not a function", callback.to_display_string()),
            });
        }
        let mut steps = 0;
        if matches!(method, CallbackMethod::Reduce | CallbackMethod::ReduceRight) {
            let backwards = matches!(method, CallbackMethod::ReduceRight);
            let bound = length as i64;
            let step: i64 = if backwards { -1 } else { 1 };
            let mut index: i64 = if backwards { bound - 1 } else { 0 };
            let mut accumulator = match arguments.get(1) {
                Some(initial) => initial.clone(),
                None => {
                    // The seed is the first position that is *there* from whichever end the walk
                    // starts: a receiver of nothing but holes has no answer to give, exactly as
                    // an empty one does not.
                    let mut seed = None;
                    while index >= 0 && index < bound {
                        Self::walk_step(&mut steps)?;
                        let at = index as u64;
                        index += step;
                        if let Some(value) = self.element_read(receiver, at, instruction)? {
                            seed = Some(value);
                            break;
                        }
                    }
                    let Some(seed) = seed else {
                        return Err(ExecutionError::EmptyReduce);
                    };
                    seed
                }
            };
            while index >= 0 && index < bound {
                Self::walk_step(&mut steps)?;
                let at = index as u64;
                index += step;
                let Some(value) = self.element_read(receiver, at, instruction)? else {
                    continue;
                };
                #[expect(clippy::cast_precision_loss)]
                let position = at as f64;
                accumulator = self.call_value(
                    &callback,
                    Value::Undefined,
                    vec![
                        accumulator,
                        value,
                        Value::Number(position),
                        receiver.clone(),
                    ],
                    instruction,
                )?;
            }
            return Ok(accumulator);
        }
        let this_value = arguments.get(1).cloned().unwrap_or(Value::Undefined);
        let backwards = matches!(
            method,
            CallbackMethod::FindLast | CallbackMethod::FindLastIndex
        );
        // The four `find` methods read every position, holes included: they were added to the
        // language after it stopped skipping them. The rest ask first.
        let visits_holes = matches!(
            method,
            CallbackMethod::Find
                | CallbackMethod::FindIndex
                | CallbackMethod::FindLast
                | CallbackMethod::FindLastIndex
        )
        // A view has no holes to skip: its methods read each position with `Get` rather than
        // asking `HasProperty` first, so the callback is called for every one of the `len`
        // positions the walk started with. The distinction only shows once a resizable buffer
        // has shrunk under the walk -- and that is exactly when skipping would end the
        // iteration early instead of handing the callback the `undefined` it is owed.
        || matches!(receiver, Value::TypedArray(_));
        let created = match method {
            CallbackMethod::Map | CallbackMethod::ParallelMap => {
                Some(self.array_species_create(receiver, length, instruction)?)
            }
            CallbackMethod::Filter | CallbackMethod::FlatMap => {
                Some(self.array_species_create(receiver, 0, instruction)?)
            }
            _ => None,
        };
        let mut written = 0;
        for order in 0..length {
            let index = if backwards { length - order - 1 } else { order };
            Self::walk_step(&mut steps)?;
            let value = if visits_holes {
                self.element_get(receiver, index, instruction)?
            } else {
                match self.element_read(receiver, index, instruction)? {
                    Some(value) => value,
                    None => continue,
                }
            };
            #[expect(clippy::cast_precision_loss)]
            let position = index as f64;
            let produced = self.call_value(
                &callback,
                this_value.clone(),
                vec![value.clone(), Value::Number(position), receiver.clone()],
                instruction,
            )?;
            match method {
                CallbackMethod::Map | CallbackMethod::ParallelMap => {
                    self.create_element(
                        created.as_ref().unwrap_or(&Value::Undefined),
                        index,
                        produced,
                        instruction,
                    )?;
                }
                CallbackMethod::Filter => {
                    if produced.is_truthy() {
                        self.create_element(
                            created.as_ref().unwrap_or(&Value::Undefined),
                            written,
                            value,
                            instruction,
                        )?;
                        written += 1;
                    }
                }
                // One level, and only of what the callback produced: `flatMap` does not flatten
                // an array that was already an element.
                CallbackMethod::FlatMap => {
                    let into = created.as_ref().unwrap_or(&Value::Undefined);
                    if matches!(unwrap_proxy(&produced), Value::Array(_)) {
                        let inner = self.array_like_length(&produced, instruction)?;
                        for at in 0..inner {
                            Self::walk_step(&mut steps)?;
                            if self.element_has(&produced, at, instruction)? {
                                let nested = self.element_get(&produced, at, instruction)?;
                                self.create_element(into, written, nested, instruction)?;
                            }
                            written += 1;
                        }
                    } else {
                        self.create_element(into, written, produced, instruction)?;
                        written += 1;
                    }
                }
                CallbackMethod::Find | CallbackMethod::FindLast => {
                    if produced.is_truthy() {
                        return Ok(value);
                    }
                }
                CallbackMethod::FindIndex | CallbackMethod::FindLastIndex => {
                    if produced.is_truthy() {
                        return Ok(Value::Number(position));
                    }
                }
                CallbackMethod::Some => {
                    if produced.is_truthy() {
                        return Ok(Value::Boolean(true));
                    }
                }
                CallbackMethod::Every if !produced.is_truthy() => {
                    return Ok(Value::Boolean(false));
                }
                _ => {}
            }
        }
        Ok(match method {
            CallbackMethod::Map | CallbackMethod::ParallelMap => {
                created.unwrap_or(Value::Undefined)
            }
            CallbackMethod::Filter | CallbackMethod::FlatMap => {
                let created = created.unwrap_or(Value::Undefined);
                self.finish_created_array(&created, written)?;
                created
            }
            CallbackMethod::FindIndex | CallbackMethod::FindLastIndex => Value::Number(-1.0),
            CallbackMethod::Some => Value::Boolean(false),
            CallbackMethod::Every => Value::Boolean(true),
            _ => Value::Undefined,
        })
    }

    /// `%ArrayIteratorPrototype%.next` over a receiver the dense storage cannot answer for.
    pub(crate) fn generic_array_iterator_next(
        &mut self,
        receiver: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let iterated = iterator_state(receiver)?;
        let Value::Object(state) = receiver else {
            unreachable!("only an object carries the marker");
        };
        let state = Rc::clone(state);
        let (index, part) = {
            let state = state.borrow();
            (
                state
                    .get("@@index")
                    .unwrap_or(Value::Number(0.0))
                    .as_number(),
                state
                    .get("@@part")
                    .unwrap_or(Value::String("value".to_owned().into()))
                    .to_display_string(),
            )
        };
        // The length is re-read every step. An array-like shortened during iteration must stop
        // early rather than read past its end, which is exactly what a cached length would do.
        let length = self.array_like_length(&iterated, instruction)?;
        let index = Self::to_length(index);
        if index >= length {
            return Ok(iteration_result(Value::Undefined, true));
        }
        #[expect(clippy::cast_precision_loss)]
        let position = index as f64;
        state
            .borrow_mut()
            .set("@@index", Value::Number(position + 1.0));
        // An iterator walks *indices*, so it visits a hole -- and what it yields there is what a
        // read yields, which is undefined unless the chain fills it.
        let value = self.element_get(&iterated, index, instruction)?;
        let yielded = match part.as_str() {
            "key" => Value::Number(position),
            "entry" => Value::Array(ArrayData::new(vec![Value::Number(position), value])),
            _ => value,
        };
        Ok(iteration_result(yielded, false))
    }

    /// `Array.prototype.sort` over a receiver that is not an array.
    ///
    /// The specification collects the positions that are there, sorts that list, writes it back
    /// over the front and deletes what is left: holes end up at the end however they were spread
    /// through the receiver, and the ordering is the same merge the dense path uses so the two
    /// cannot disagree about where a comparator's ties land.
    pub(crate) fn generic_array_sort(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let comparator = arguments.first().cloned().unwrap_or(Value::Undefined);
        if !matches!(comparator, Value::Undefined) && !is_callable(&comparator) {
            return Err(comparator_refused());
        }
        let length = self.array_like_length(receiver, instruction)?;
        let mut steps = 0;
        let mut present = Vec::new();
        for index in 0..length {
            Self::walk_step(&mut steps)?;
            if let Some(value) = self.element_read(receiver, index, instruction)? {
                present.push(value);
            }
        }
        let counted = present.len() as u64;
        let scratch = Value::Array(ArrayData::new(present));
        self.array_sort(&scratch, arguments, instruction)?;
        let Value::Array(ordered) = &scratch else {
            unreachable!("just built as an array");
        };
        let ordered = ordered.borrow().clone();
        for (index, value) in ordered.into_iter().enumerate() {
            self.element_set(receiver, index as u64, value, instruction)?;
        }
        let mut index = counted;
        while index < length {
            Self::walk_step(&mut steps)?;
            self.element_delete(receiver, index, instruction)?;
            index += 1;
        }
        Ok(receiver.clone())
    }

    /// Whether a callback method hands its callback the receiver itself rather than a copy.
    ///
    /// Every one of these passes the object it is walking as the callback's last argument and
    /// reads each element at the moment it visits it. A view handed to one got a *snapshot*
    /// instead, and that is visible twice over: a write the callback performs on the view --
    /// `a[i + 1] = 1`, which is the whole of what Test262's `values-are-not-cached` files do --
    /// was invisible to the rest of the walk, and the third argument was an `Array` that a
    /// program could tell apart from the view it had called the method on.
    ///
    /// `sort` is not one of them. Its comparator is handed two elements and never the receiver,
    /// and the order a view sorts into with no comparator is numeric where an array's is
    /// lexicographic -- so it keeps the copy that makes those two different sorts, and writes
    /// the result back.
    const fn callback_walks_live(method: CallbackMethod) -> bool {
        !matches!(
            method,
            CallbackMethod::Sort
                | CallbackMethod::GeneratorNext
                | CallbackMethod::GeneratorReturn
                | CallbackMethod::GeneratorThrow
                | CallbackMethod::AsyncGeneratorNext
                | CallbackMethod::AsyncGeneratorReturn
                | CallbackMethod::AsyncGeneratorThrow
        )
    }

    /// The receiver an array method should walk, when one is being redirected onto something
    /// that is not an array.
    ///
    /// `Array.prototype.slice.call(arguments)` and `[].forEach.call(nodeList, ...)` are why the
    /// array methods are generic, and redirection is the only way one of them can meet a
    /// non-array receiver: reading `slice` off a plain object finds nothing, so `obj.slice()` is
    /// a `TypeError` here exactly as it is in node. A string walks as its characters; anything
    /// with a `length` walks as its indexed properties. The copy is not written back, so a
    /// mutating method redirected this way changes nothing -- which is visible in the untouched
    /// object rather than hidden in a wrong result.
    pub(crate) fn as_array_like(
        &mut self,
        callee: &Value,
        receiver: Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let walks_an_array = match callee {
            // The three generator methods are not walks: each resumes a body, and a receiver
            // turned into a snapshot of its elements is not a generator any more. The async one
            // was missing from this list, so reading `g.next` off an async generator and calling
            // it as a value -- which is exactly what any code driving the protocol by hand does
            // -- coerced the generator to an array and then complained the array was not callable.
            Value::Callback(method, _) => !matches!(
                method,
                CallbackMethod::GeneratorNext
                    | CallbackMethod::GeneratorReturn
                    | CallbackMethod::GeneratorThrow
                    | CallbackMethod::AsyncGeneratorNext
                    | CallbackMethod::AsyncGeneratorReturn
                    | CallbackMethod::AsyncGeneratorThrow
            ),
            Value::Native(native, _) => native.walks_an_array(),
            _ => false,
        };
        let preserves_live_typed_array = matches!(receiver, Value::TypedArray(_))
            && match callee {
                Value::Native(native, _) => native.creates_array_iterator(),
                Value::Callback(method, _) => Self::callback_walks_live(*method),
                _ => false,
            };
        // `ValidateTypedArray`, which every `%TypedArray%.prototype` method performs before it
        // does anything else -- including before it coerces its arguments. This is the choke
        // point for it, because this is where a typed array receiver is recognised: the methods
        // themselves are `Array.prototype`'s, and each one growing its own guard would be a
        // dozen chances to forget. It sits ahead of the early return so the iterator methods,
        // which keep the live view rather than a snapshot, are checked too.
        // `join`'s default spelling and `copyWithin` do not walk a snapshot -- they read the
        // receiver directly -- so they are named here rather than inherited from the list above.
        let validates_typed_array = walks_an_array
            || matches!(
                callee,
                Value::Native(Native::ArrayJoinDefault | Native::ArrayCopyWithin, _)
            );
        if validates_typed_array
            && let Value::TypedArray(view) = &receiver
            && view.buffer.detached()
        {
            return Err(ExecutionError::BufferDetached);
        }
        // The methods with a generic implementation walk the receiver itself. A typed array is
        // the exception: its elements are bytes in a shared buffer rather than properties, and
        // the copy-and-write-back below is what turns an array method into one of its own.
        if Self::walks_a_live_receiver(callee) && !matches!(receiver, Value::TypedArray(_)) {
            return self.as_receiver_object(receiver);
        }
        if !walks_an_array || matches!(receiver, Value::Array(_)) || preserves_live_typed_array {
            return Ok(receiver);
        }
        let items = match &receiver {
            // A typed array already *is* the list; reading it through `length` and indices would
            // work too, and this skips the round trip.
            Value::TypedArray(view) => view.items(),
            Value::String(text) => text
                .units()
                .map(|unit| Value::String(StringUnits::from_units(&[unit])))
                .collect(),
            Value::Object(_) | Value::Function(_) => {
                // `LengthOfArrayLike` is `ToLength(Get(obj, "length"))`, and the conversion is
                // the program's own: `{length: {valueOf(){throw x}}}` throws here, and a
                // `length` that is an object read as `NaN` -- an empty walk -- without it.
                let raw = self.read_property(&receiver, "length", instruction)?;
                let length = bounded_length(self.as_number_argument(Some(&raw), instruction)?)?;
                let mut items = Vec::with_capacity(length);
                for index in 0..length {
                    items.push(self.read_property(&receiver, &index.to_string(), instruction)?);
                }
                items
            }
            // Step one of every one of these is `ToObject(this value)`, and that is where
            // `null` and `undefined` stop: there is no object to walk, and an empty walk would
            // report a redirection onto nothing as a redirection onto an empty array.
            Value::Undefined | Value::Null => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!(
                        "Array.prototype method called on {}",
                        receiver.to_display_string()
                    ),
                });
            }
            // A number, a boolean, a symbol: each converts to a wrapper with no indexed
            // properties, so the walk really is empty -- which is what
            // `Array.prototype.map.call(1, f)` gives.
            _ => Vec::new(),
        };
        Ok(Value::Array(ArrayData::new(items)))
    }
}
