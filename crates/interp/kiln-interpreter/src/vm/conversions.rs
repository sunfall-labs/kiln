use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// `new ArrayBuffer(byteLength, options?)`.
    ///
    /// The options bag is what makes a buffer resizable, and it is read the way the
    /// specification reads it: only an *object* second argument is consulted at all, and only a
    /// `maxByteLength` that is present and not `undefined` opts in. That read is an ordinary
    /// property get -- it can run a getter, and a test that counts how many times it runs is
    /// watching -- which is why this lives on the machine rather than in `call_native`.
    ///
    /// A ceiling below the requested length is a `RangeError` rather than a silent clamp: the
    /// program asked for a buffer that cannot exist, and quietly building a different one hides
    /// the mistake until something reads past the end.
    pub(super) fn construct_array_buffer(
        &mut self,
        shared: bool,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        // `as_number_argument`, not the pure `to_index`: a length that is an object is reduced
        // by *calling* its `valueOf`, and a symbol or a BigInt is a `TypeError`. The pure form
        // answered `NaN` for an object, which `to_index` then read as zero -- so
        // `new ArrayBuffer({valueOf: () => 12})` was a *nought-byte* buffer, silently, and the
        // program found out somewhere else entirely.
        let requested = self.as_number_argument(arguments.first(), instruction)?;
        let length = bounded_buffer(to_index_number(requested)?, 1)?;
        let max = match arguments.get(1) {
            Some(options @ (Value::Object(_) | Value::Array(_) | Value::Function(_))) => {
                match self.read_property(options, "maxByteLength", instruction)? {
                    Value::Undefined => None,
                    requested => {
                        let number = self.as_number_argument(Some(&requested), instruction)?;
                        Some(to_index_number(number)?)
                    }
                }
            }
            _ => None,
        };
        let Some(max) = max else {
            return Ok(Value::ArrayBuffer(Rc::new(BufferData {
                bytes: Bytes::new(shared, vec![0; length]),
                max_byte_length: None,
                shared,
                detached: std::cell::Cell::new(false),
                immutable: false,
                properties: RefCell::new(ObjectData::default()),
            })));
        };
        // The ceiling is what a later `resize` may reach, so it is the figure the allocation
        // bound has to be applied to -- otherwise a program could reserve unbounded memory one
        // `resize` at a time from a buffer that started out tiny. A ceiling above the bound is a
        // catchable `RangeError` rather than the uncatchable refusal an oversized *allocation*
        // gets, because nothing has been allocated: the specification says a data block that
        // cannot be created is a `RangeError`, and a declared ceiling this realm cannot honour
        // is exactly that.
        if max > MAX_BUFFER_BYTES {
            return Err(ExecutionError::BufferLengthOutOfRange { length: max });
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let max = max as usize;
        if length > max {
            return Err(ExecutionError::BufferLengthOutOfRange {
                length: length as f64,
            });
        }
        Ok(Value::ArrayBuffer(Rc::new(BufferData {
            bytes: Bytes::new(shared, vec![0; length]),
            max_byte_length: Some(max),
            shared,
            detached: std::cell::Cell::new(false),
            immutable: false,
            properties: RefCell::new(ObjectData::default()),
        })))
    }

    /// `ToIndex` on an argument, with the coercion only the machine can perform.
    ///
    /// Named `as_` rather than `to_`, like [`Vm::as_primitive`] and for the same reason: it
    /// takes the machine by mutable reference because coercing an object *runs* its code.
    ///
    /// [`to_index`] reads the value it is handed; an argument that is an object has first to be
    /// reduced to a primitive, and reducing one *calls a method* -- which is exactly the hook
    /// every ordering test in this area uses to resize the buffer mid-construction.
    pub(super) fn as_index_argument(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<usize, ExecutionError> {
        let number = self.as_number_argument(value, instruction)?;
        let index = to_index_number(number)?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(index as usize)
    }

    /// The arguments of a typed-array constructor, with the numeric ones reduced.
    ///
    /// Which arguments are numeric depends on the first: over a buffer they are `byteOffset` and
    /// `length`, and over anything that is not an object it is the length alone -- an object
    /// source is a list or an iterable, and is not converted at all. Reducing one *calls* the
    /// program's `valueOf`, which is why this takes the machine and why the pure conversion
    /// inside `construct_typed_array` could not do it: it answered `NaN` for an object, and
    /// `to_index` read that as zero, so `new Uint8Array(buffer, {valueOf: () => 4})` silently
    /// viewed from offset *nought* rather than four.
    pub(super) fn typed_array_arguments(
        &mut self,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Vec<Value>, ExecutionError> {
        let mut coerced = arguments.to_vec();
        match arguments.first() {
            Some(Value::ArrayBuffer(_)) => {
                for slot in [1usize, 2usize] {
                    let Some(value) = coerced.get(slot).cloned() else {
                        continue;
                    };
                    if matches!(value, Value::Undefined) {
                        continue;
                    }
                    let number = self.as_number_argument(Some(&value), instruction)?;
                    coerced[slot] = Value::Number(number);
                }
            }
            Some(first) if !is_an_object(first) && !is_callable(first) => {
                let first = first.clone();
                let number = self.as_number_argument(Some(&first), instruction)?;
                coerced[0] = Value::Number(number);
            }
            _ => {}
        }
        Ok(coerced)
    }

    /// `ToNumber` on an argument, with the coercion only the machine can perform and the two
    /// refusals the specification makes.
    ///
    /// A symbol and a BigInt are `TypeError`s rather than numbers. The pure [`Value::as_number`]
    /// answers `NaN` for one and the BigInt's magnitude for the other, and either would be
    /// written into the buffer as though the program had asked for it -- a silent wrong answer
    /// where the language promises a throw.
    pub(super) fn as_number_argument(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<f64, ExecutionError> {
        let primitive = match value {
            Some(value) => self.as_primitive(value, PrimitiveHint::Number, instruction)?,
            None => Value::Undefined,
        };
        match primitive {
            Value::Symbol(_) => Err(ExecutionError::SymbolIsNotANumber),
            Value::BigInt(_) => Err(ExecutionError::BigIntIsNotANumber),
            other => Ok(other.as_number()),
        }
    }

    /// `ToNumber` or `ToBigInt` on a value being written through a typed array element,
    /// whichever numeric tower the element kind belongs to.
    ///
    /// The machine has to be the one to do it: the value may be an object, and reducing one to
    /// a primitive *runs its `valueOf`* -- which is the hook every ordering test in this area
    /// uses to resize the buffer out from under the write that follows.
    pub(super) fn as_element(
        &mut self,
        kind: TypedKind,
        value: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let primitive = self.as_primitive(value, PrimitiveHint::Number, instruction)?;
        coerce_element(kind, &primitive)
    }

    /// `ToBigInt`, which accepts far less than `ToNumber` does.
    ///
    /// A number is refused rather than converted -- `view.setBigInt64(0, 1)` is a `TypeError`,
    /// not a one -- because an implicit crossing between the two numeric types is precisely what
    /// having two of them is meant to prevent.
    pub(super) fn as_big_int_argument(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<num_bigint::BigInt, ExecutionError> {
        let primitive = match value {
            Some(value) => self.as_primitive(value, PrimitiveHint::Number, instruction)?,
            None => Value::Undefined,
        };
        match primitive {
            Value::BigInt(value) => Ok((*value).clone()),
            Value::Boolean(flag) => Ok(num_bigint::BigInt::from(u8::from(flag))),
            // A string that is not an integer literal is a `SyntaxError` rather than a NaN,
            // which is the same answer `BigInt("x")` gives: this type has no NaN to become.
            Value::String(text) => match text.to_lossy_string().trim() {
                "" => Ok(num_bigint::BigInt::from(0)),
                trimmed => trimmed
                    .parse::<num_bigint::BigInt>()
                    .map_err(|_| ExecutionError::InvalidBigIntString),
            },
            _ => Err(ExecutionError::NotABigInt),
        }
    }
}

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// `ToPrimitive`: reduce an object to a value the operators can work on.
    ///
    /// Named `as_primitive` rather than `to_primitive` because it takes `&mut self`: reducing an
    /// object *runs* one of its methods, which is the whole reason it lives here.
    ///
    /// An object becomes a primitive by *calling* one of its own methods, which is why this is on
    /// the machine and why `"" + obj` cannot be decided by looking at the value alone. `valueOf`
    /// is tried first unless a string is wanted, which is the order the specification gives and
    /// the reason `new Date() - 0` is a number while `` `${date}` `` is a sentence.
    ///
    /// A method that hands back another object is skipped rather than used: the point is to reach
    /// a primitive, and one that does not is no closer.
    pub(super) fn as_primitive(
        &mut self,
        value: &Value,
        hint: PrimitiveHint,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        // A `Temporal` value is an object in the specification's sense, and reducing one is a
        // *call*: every `Temporal` type's `valueOf` throws on purpose, so that `date1 < date2`
        // cannot quietly compare two NaNs and answer false in both directions. Passing it
        // through as its own primitive is what made `Number(date)` answer NaN.
        if !matches!(
            value,
            Value::Object(_)
                | Value::Function(_)
                | Value::Array(_)
                | Value::Shared(_)
                | Value::Temporal(_)
                // A `Date` is a value kind here rather than an object, but it reduces by
                // *calling* for the same reason a `Temporal` does, and it is the one built-in
                // where the answer depends on which hint was asked for. Passing it through as
                // its own primitive is what made `date + date` add two instants.
                | Value::Date(_)
                | Value::RegExp(_)
        ) && !matches!(value, Value::TypedArray(view) if is_buffer_view(view))
        {
            // A typed array reduces to its element list without a call -- except a Buffer,
            // whose `toString` is the decoder on `Buffer.prototype`: `String(buf)`, a template
            // and `[buf].join()` all read the text, as node does.
            return Ok(value.clone());
        }
        // `@@toPrimitive` overrides both and is told which hint it is answering. The hint spelling
        // matches the C backend's exactly -- "string" or "default" -- so a program that branches
        // on it cannot tell the two engines apart. `Number(x)` passing "default" rather than
        // "number" is a shared inaccuracy, not a divergence, and is noted in NOTES.md.
        let exotic =
            self.read_property(value, WellKnown::ToPrimitive.property_key(), instruction)?;
        // `GetMethod` refuses a value that is neither callable nor nullish, and it refuses it
        // *here* rather than falling through: a program that wrote `@@toPrimitive = 1` said how
        // this value reduces, and quietly reducing it some other way answered
        // "[object Object]" where the language throws.
        if !exotic.is_nullish() && !is_callable(&exotic) {
            return Err(ExecutionError::NotCallable {
                instruction,
                value: exotic.type_name(),
            });
        }
        if is_callable(&exotic) {
            let hint = Value::String(hint.spelling().to_owned().into());
            let produced =
                self.call_value(&exotic, value.clone(), Vec::from([hint]), instruction)?;
            // An `@@toPrimitive` that answers with another object ends the conversion rather
            // than being skipped: the program said this is how the value reduces, and falling
            // through to `valueOf` would answer with something it deliberately overrode.
            return if matches!(
                produced,
                Value::Object(_) | Value::Function(_) | Value::Array(_) | Value::Shared(_)
            ) {
                Err(ExecutionError::NotPrimitive)
            } else {
                Ok(produced)
            };
        }
        let order = if hint == PrimitiveHint::String {
            ["toString", "valueOf"]
        } else {
            ["valueOf", "toString"]
        };
        for name in order {
            let method = self.read_property(value, name, instruction)?;
            if !matches!(
                method,
                Value::Function(_) | Value::Native(_, _) | Value::Callback(_, _)
            ) {
                continue;
            }
            let result = self.call_value(&method, value.clone(), Vec::new(), instruction)?;
            // A `valueOf` that hands back the receiver -- `Object.prototype.valueOf` on a
            // typed array -- has not answered; the walk goes on to `toString`.
            if !matches!(
                result,
                Value::Object(_) | Value::Function(_) | Value::Array(_) | Value::TypedArray(_)
            ) {
                return Ok(result);
            }
        }
        // Nothing answered with a primitive, which the specification ends the program over.
        Err(ExecutionError::NotPrimitive)
    }
}
