use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// `Date.prototype.toJSON`, which is specified against any object.
    ///
    /// Answers null rather than throwing for an instant with no ISO spelling, which is the one
    /// difference from `toISOString` and the whole reason both exist: a document containing an
    /// invalid date still serialises.
    pub(super) fn date_to_json(
        &mut self,
        receiver: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let primitive = match receiver {
            // A date is a value kind here rather than an object, so `ToPrimitive` on one with a
            // number hint is the instant it holds.
            Value::Date(instant) => Value::Number(instant.time.get()),
            other => self.as_primitive(other, PrimitiveHint::Number, instruction)?,
        };
        if matches!(primitive, Value::Number(number) if !number.is_finite()) {
            return Ok(Value::Null);
        }
        let to_iso = self.read_property(receiver, "toISOString", instruction)?;
        if !is_callable(&to_iso) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "toISOString is not a function".to_owned(),
            });
        }
        self.call_value(&to_iso, receiver.clone(), Vec::new(), instruction)
    }

    /// `Date.prototype[Symbol.toPrimitive]`.
    ///
    /// The one built-in `@@toPrimitive` the language ships, and the reason a date prefers its
    /// string form: an unrecognised hint is a `TypeError` rather than a guess, and the "default"
    /// hint reduces to the sentence rather than to the number -- which is what makes
    /// `` `${date}` `` a date and `date - 0` a count of milliseconds.
    pub(super) fn date_to_primitive(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if !matches!(
            receiver,
            Value::Object(_)
                | Value::Function(_)
                | Value::Array(_)
                | Value::Date(_)
                | Value::Promise(_)
                | Value::RegExp(_)
                | Value::Proxy(_)
                | Value::Native(_, _)
                | Value::Bound(_)
                | Value::Callback(_, _)
        ) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Date.prototype[Symbol.toPrimitive] called on a non-object".to_owned(),
            });
        }
        let prefer_string = match arguments.first() {
            Some(Value::String(hint)) if hint == "string" || hint == "default" => true,
            Some(Value::String(hint)) if hint == "number" => false,
            other => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!(
                        "Invalid hint: {}",
                        other.map_or_else(|| "undefined".to_owned(), Value::to_display_string)
                    ),
                });
            }
        };
        // `OrdinaryToPrimitive`, spelled out rather than deferred to `as_primitive`: that one
        // begins by looking for this very method, so calling it here would recur.
        let order = if prefer_string {
            ["toString", "valueOf"]
        } else {
            ["valueOf", "toString"]
        };
        for name in order {
            let method = self.read_property(receiver, name, instruction)?;
            if !is_callable(&method) {
                continue;
            }
            let produced = self.call_value(&method, receiver.clone(), Vec::new(), instruction)?;
            if !matches!(
                produced,
                Value::Object(_) | Value::Function(_) | Value::Array(_)
            ) {
                return Ok(produced);
            }
        }
        Err(ExecutionError::NotPrimitive)
    }
}

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// One method on `Date.prototype`.
    ///
    /// On the machine rather than in [`call_native`] because the specification's order is
    /// observable. A setter takes the receiver's instant *first* and only then converts its
    /// arguments, left to right, and each of those conversions can run a `valueOf` that reaches
    /// the same date. The pure `as_number` could do none of it: it answered `NaN` for every
    /// object, which turned `d.setMonth({valueOf: () => 1})` into a silently invalid date.
    pub(super) fn date_dispatch(
        &mut self,
        method: DateMethod,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        match method {
            // The two generic methods, specified against any object rather than against a date.
            // That is not a technicality: it is what lets `JSON.stringify` reach `toJSON` on a
            // value that merely answers `toISOString`, and what makes
            // `Date.prototype[Symbol.toPrimitive].call(plain, "string")` work.
            DateMethod::ToJson => return self.date_to_json(receiver, instruction),
            DateMethod::ToPrimitive => {
                return self.date_to_primitive(receiver, arguments, instruction);
            }
            _ => {}
        }
        let Value::Date(instant) = receiver else {
            return Err(ExecutionError::TypeErrorMessage {
                message: "this is not a Date object.".to_owned(),
            });
        };
        // Read before anything below converts an argument, because a conversion can run user
        // code that moves this very date.
        let time = instant.time.get();
        if let Some(field) = method.writes() {
            // Only the parameters the method declares, and beyond the first only those the
            // caller supplied: `setMilliseconds(a, b)` never touches `b`, so a `valueOf` on it
            // must not run. The first is converted either way -- it is not optional, which is
            // what makes `d.setHours()` NaN rather than a no-op.
            let declared = method.length() as usize;
            let converted = declared.min(arguments.len()).max(1);
            let mut supplied = Vec::with_capacity(converted);
            for index in 0..converted {
                supplied.push(self.as_number_argument(arguments.get(index), instruction)?);
            }
            return Ok(Value::Number(
                match date_written(method, field, time, &supplied) {
                    Some(updated) => {
                        instant.time.set(updated);
                        updated
                    }
                    None => f64::NAN,
                },
            ));
        }
        if let Some(field) = method.reads() {
            return Ok(Value::Number(date_read(field, time)));
        }
        Ok(match method {
            // Annex B's reader, which is the year less 1900 rather than a two-digit year: 2026
            // reads as 126, which is the shape that made the whole family legacy.
            DateMethod::GetYear => Value::Number(match date_parts(time) {
                Some(parts) => parts.year as f64 - 1900.0,
                None => f64::NAN,
            }),
            DateMethod::ToIsoString => match iso_string(time) {
                Some(text) => Value::String(text.into()),
                // An instant with no ISO spelling is a RangeError, not a placeholder string.
                None => return Err(ExecutionError::DateOutOfRange),
            },
            DateMethod::ToString | DateMethod::ToLocaleString => {
                Value::String(date_to_string(time).into())
            }
            DateMethod::ToDateString | DateMethod::ToLocaleDateString => {
                Value::String(date_only_string(time).into())
            }
            DateMethod::ToTimeString | DateMethod::ToLocaleTimeString => {
                Value::String(time_only_string(time).into())
            }
            DateMethod::ToUtcString => Value::String(utc_string(time).into()),
            _ => unreachable!("readers, writers and the generic pair are handled above"),
        })
    }
}
