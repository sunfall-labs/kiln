use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// `IsRegExp(value)`.
    ///
    /// Asks the value, not the engine: an object whose `@@match` is truthy *is* a regexp for
    /// every purpose the language cares about here, and one that has set it to `false` is not,
    /// even if this realm built it. That is the escape hatch a library uses to pass a regexp to
    /// `replaceAll` without being told it is not global.
    pub(super) fn is_regexp(
        &mut self,
        value: &Value,
        instruction: usize,
    ) -> Result<bool, ExecutionError> {
        if !is_reflect_target(value) {
            return Ok(false);
        }
        let matcher = self.read_property(value, WellKnown::Match.property_key(), instruction)?;
        if !matches!(matcher, Value::Undefined) {
            return Ok(matcher.is_truthy());
        }
        Ok(matches!(value, Value::RegExp(_)))
    }

    /// `RegExpExec(R, S)`: match `subject` through whatever `exec` the program can see.
    ///
    /// The lookup is the whole point. A program that puts its own `exec` on a regexp -- or on an
    /// object that is not one at all -- has redefined what matching *means* for that value, and
    /// all five of the symbol-keyed methods are written in terms of this so that one
    /// redefinition reaches every one of them. That is also what makes them work on a receiver
    /// this engine never built: none of them needs a `[[RegExpMatcher]]`, only an `exec`.
    pub(super) fn regexp_exec_abstract(
        &mut self,
        target: &Value,
        subject: &str,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        self.regexp_exec_abstract_value(
            target,
            Value::String(subject.to_owned().into()),
            instruction,
        )
    }

    pub(super) fn regexp_exec_abstract_value(
        &mut self,
        target: &Value,
        subject: Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let exec = self.read_property(target, "exec", instruction)?;
        if is_callable(&exec) {
            let result =
                self.call_value(&exec, target.clone(), vec![subject.clone()], instruction)?;
            // Null or an object and nothing else: a program whose `exec` answers a number has
            // broken the protocol, and reading `"0"` off it would silently produce undefined.
            if !matches!(result, Value::Null) && !is_reflect_target(&result) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "RegExp exec method returned something other than an Object or null"
                        .to_owned(),
                });
            }
            return Ok(result);
        }
        // No callable `exec` in the way, so the built-in matcher answers -- and it is the one
        // step that does need a real regexp.
        if !matches!(target, Value::RegExp(_)) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Method called on an incompatible receiver".to_owned(),
            });
        }
        self.dispatch_native(Native::RegExpExec, target, &[subject], instruction)
    }

    /// Read one property and coerce it to text, which several of the steps below need in a row.
    pub(super) fn read_text(
        &mut self,
        target: &Value,
        name: &str,
        instruction: usize,
    ) -> Result<String, ExecutionError> {
        let value = self.read_property(target, name, instruction)?;
        self.as_text(&value, instruction)
    }

    /// `ToLength` of one property: the count a result object claims to have.
    pub(super) fn read_length(
        &mut self,
        target: &Value,
        name: &str,
        instruction: usize,
    ) -> Result<usize, ExecutionError> {
        let value = self.read_property(target, name, instruction)?;
        let number = self.as_number_argument(Some(&value), instruction)?;
        Ok(to_length(number))
    }

    pub(super) fn set_last_index(
        &mut self,
        target: &Value,
        index: f64,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        // `Set(R, "lastIndex", v, true)` -- a *throwing* write, which is the whole reason the
        // specification spells the last argument. A refused write that is merely swallowed
        // leaves a `g` cursor where it was, and the loop that advances it never terminates.
        self.assign_with_strict(
            target,
            &Value::String("lastIndex".to_owned().into()),
            Value::Number(index),
            instruction,
            true,
        )
    }

    /// The five `RegExp.prototype` methods the language reaches through a well-known symbol.
    ///
    /// Every one of them is *generic*: the specification requires only that the receiver be an
    /// object, and does all its matching through [`Vm::regexp_exec_abstract`]. That is what makes
    /// `"abc".replace(fake, "x")` work when `fake` is a plain object with an `exec` and a
    /// `@@replace` inherited from `RegExp.prototype` -- the idiom a library uses to intercept
    /// string methods without subclassing.
    pub(super) fn regexp_symbol(
        &mut self,
        method: RegExpSymbol,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if !is_reflect_target(receiver) {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!(
                    "RegExp.prototype{} called on a value that is not an object",
                    method.js_name()
                ),
            });
        }
        let subject_argument = arguments.first().unwrap_or(&Value::Undefined);
        let second = arguments.get(1).cloned().unwrap_or(Value::Undefined);
        if method == RegExpSymbol::MatchAll {
            let subject = self.as_text_units(subject_argument, instruction)?;
            let units = subject.to_units();
            return self.regexp_match_all_symbol(receiver, &units, instruction);
        }
        if method == RegExpSymbol::Replace {
            let subject = self.as_text_units(subject_argument, instruction)?;
            return self.regexp_replace_symbol(receiver, &subject, &second, instruction);
        }
        let subject = self.as_text(subject_argument, instruction)?;
        match method {
            RegExpSymbol::Match => self.regexp_match_symbol(receiver, &subject, instruction),
            RegExpSymbol::Search => self.regexp_search_symbol(receiver, &subject, instruction),
            RegExpSymbol::MatchAll => unreachable!("handled with UTF-16 units above"),
            RegExpSymbol::Replace => unreachable!("replace uses lossless string units above"),
            RegExpSymbol::Split => {
                self.regexp_split_symbol(receiver, &subject, &second, instruction)
            }
        }
    }

    /// `RegExp.prototype[@@match]`.
    pub(super) fn regexp_match_symbol(
        &mut self,
        target: &Value,
        subject: &str,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let flags = self.read_text(target, "flags", instruction)?;
        // Without `g` this *is* `exec`: one result, with its captures. The array of texts is
        // what `g` means, and reading the flag off the receiver rather than off an internal slot
        // is what lets a program turn one into the other.
        if !flags.contains('g') {
            return self.regexp_exec_abstract(target, subject, instruction);
        }
        let full_unicode = flags.contains('u') || flags.contains('v');
        self.set_last_index(target, 0.0, instruction)?;
        let units: Vec<u16> = subject.encode_utf16().collect();
        let mut found = Vec::new();
        loop {
            let result = self.regexp_exec_abstract(target, subject, instruction)?;
            if matches!(result, Value::Null) {
                return Ok(if found.is_empty() {
                    Value::Null
                } else {
                    Value::Array(ArrayData::new(found))
                });
            }
            let matched_value = self.read_property(&result, "0", instruction)?;
            let matched = self.as_text_units(&matched_value, instruction)?;
            let empty = matched.is_empty();
            found.push(Value::String(matched.into()));
            // An empty match would leave the cursor where it is and match again forever, so the
            // walk moves it on by hand -- by a whole code point when the pattern is Unicode-aware.
            if empty {
                let index = self.read_length(target, "lastIndex", instruction)?;
                let next = advance_string_index(&units, index, full_unicode);
                self.set_last_index(target, next as f64, instruction)?;
            }
        }
    }

    /// `RegExp.prototype[@@search]`.
    ///
    /// The cursor is saved and put back, because `search` is defined not to move it: a program
    /// that searches a `g` regexp in the middle of a `while (re.exec(s))` loop must not have the
    /// loop's position taken away from it.
    pub(super) fn regexp_search_symbol(
        &mut self,
        target: &Value,
        subject: &str,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let previous = self.read_property(target, "lastIndex", instruction)?;
        if !same_value(&previous, &Value::Number(0.0)) {
            self.set_last_index(target, 0.0, instruction)?;
        }
        let result = self.regexp_exec_abstract(target, subject, instruction)?;
        let current = self.read_property(target, "lastIndex", instruction)?;
        // Written back only when it actually moved, so a receiver whose `lastIndex` is
        // non-writable is not made to refuse a write that would have changed nothing.
        if !same_value(&current, &previous) {
            self.set_computed(
                target,
                &Value::String("lastIndex".to_owned().into()),
                previous,
                instruction,
            )?;
        }
        if matches!(result, Value::Null) {
            return Ok(Value::Number(-1.0));
        }
        self.read_property(&result, "index", instruction)
    }

    /// `RegExp.prototype[@@matchAll]`.
    ///
    /// The iterator walks a *clone* of the receiver, which is the whole reason the method is not
    /// simply `@@match` with `g`: the program's regexp keeps its own `lastIndex`, so the same
    /// regexp can drive two independent walks and neither disturbs the other.
    pub(super) fn regexp_match_all_symbol(
        &mut self,
        target: &Value,
        subject: &[u16],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let flags = self.read_text(target, "flags", instruction)?;
        let matcher = self.regexp_species_construct(target, &flags, instruction)?;
        let index = self.read_length(target, "lastIndex", instruction)?;
        self.set_last_index(&matcher, index as f64, instruction)?;
        let mut iterator = ObjectData::default();
        iterator.set_non_enumerable(MATCH_ALL_REGEXP, matcher);
        iterator.set_non_enumerable(
            MATCH_ALL_SUBJECT,
            Value::String(StringUnits::from_units(subject)),
        );
        iterator.set_non_enumerable(MATCH_ALL_GLOBAL, Value::Boolean(flags.contains('g')));
        iterator.set_non_enumerable(
            MATCH_ALL_UNICODE,
            Value::Boolean(flags.contains('u') || flags.contains('v')),
        );
        iterator.set_non_enumerable(HELPER_DONE, Value::Boolean(false));
        iterator.set_prototype(Value::Native(
            Native::PrototypeOf(PrototypeTable::IteratorFor(IteratorKind::RegExpString)),
            RealmId::PRIMARY,
        ));
        Ok(Value::Object(Rc::new(RefCell::new(iterator))))
    }

    /// One step of the iterator `@@matchAll` produced.
    pub(super) fn regexp_string_iterator_next(
        &mut self,
        receiver: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Object(state) = receiver else {
            return Err(ExecutionError::NotAnIterator);
        };
        let state = Rc::clone(state);
        if state.borrow().get_own(MATCH_ALL_REGEXP).is_none() {
            return Err(ExecutionError::NotAnIterator);
        }
        if helper_slot(&state, HELPER_DONE).is_truthy() {
            return Ok(iteration_result(Value::Undefined, true));
        }
        let matcher = helper_slot(&state, MATCH_ALL_REGEXP);
        let Value::String(subject) = helper_slot(&state, MATCH_ALL_SUBJECT) else {
            return Err(ExecutionError::NotAnIterator);
        };
        let finish = |state: &Rc<RefCell<ObjectData>>| {
            state
                .borrow_mut()
                .set_non_enumerable(HELPER_DONE, Value::Boolean(true));
            Ok(iteration_result(Value::Undefined, true))
        };
        let result = match self.regexp_exec_abstract_value(
            &matcher,
            Value::String(subject.clone()),
            instruction,
        ) {
            Ok(result) => result,
            // A failed step ends the walk as surely as a finished one: a second `next` must not
            // reach back into a matcher that has already thrown.
            Err(error) => {
                state
                    .borrow_mut()
                    .set_non_enumerable(HELPER_DONE, Value::Boolean(true));
                return Err(error);
            }
        };
        if matches!(result, Value::Null) {
            return finish(&state);
        }
        // Without `g` there is exactly one match, and the walk is over as soon as it is handed
        // on -- which is what makes `matchAll` on a non-global regexp a one-element sequence
        // rather than an endless repetition of the same match.
        if !helper_slot(&state, MATCH_ALL_GLOBAL).is_truthy() {
            state
                .borrow_mut()
                .set_non_enumerable(HELPER_DONE, Value::Boolean(true));
            return Ok(iteration_result(result, false));
        }
        let matched = self.read_text(&result, "0", instruction)?;
        if matched.is_empty() {
            let units = subject.to_units();
            let index = self.read_length(&matcher, "lastIndex", instruction)?;
            let unicode = helper_slot(&state, MATCH_ALL_UNICODE).is_truthy();
            let next = advance_string_index(&units, index, unicode);
            self.set_last_index(&matcher, next as f64, instruction)?;
        }
        Ok(iteration_result(result, false))
    }

    /// `SpeciesConstructor(rx, %RegExp%)` applied to build a second regexp over the same pattern.
    pub(super) fn regexp_species_construct(
        &mut self,
        target: &Value,
        flags: &str,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let constructor = self.read_property(target, "constructor", instruction)?;
        let species = if matches!(constructor, Value::Undefined) {
            Value::Native(Native::RegExpConstructor, RealmId::PRIMARY)
        } else if !is_reflect_target(&constructor) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "constructor is not an object".to_owned(),
            });
        } else {
            let found =
                self.read_property(&constructor, WellKnown::Species.property_key(), instruction)?;
            if found.is_nullish() {
                Value::Native(Native::RegExpConstructor, RealmId::PRIMARY)
            } else {
                found
            }
        };
        if !is_callable(&species) {
            return Err(ExecutionError::NotCallable {
                instruction,
                value: species.type_name(),
            });
        }
        let arguments = vec![target.clone(), Value::String(flags.to_owned().into())];
        self.construct_value(species.clone(), arguments, species, instruction)
    }

    /// `RegExp.prototype[@@replace]`.
    ///
    /// The machine handles the regexp and function-replacement forms, while plain string
    /// replacement remains with the pure built-ins.
    pub(super) fn regexp_replace_symbol(
        &mut self,
        target: &Value,
        subject: &StringUnits,
        replacement: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let units = subject.to_units().into_vec();
        let functional = is_callable(replacement);
        // Coerced once, up front, and only when it is not a function: the template is read for
        // every match, and a `toString` that ran per match would be observable.
        let template = if functional {
            StringUnits::from("")
        } else {
            self.as_text_units(replacement, instruction)?
        };
        let flags = self.read_text(target, "flags", instruction)?;
        let global = flags.contains('g');
        let full_unicode = flags.contains('u') || flags.contains('v');
        if global {
            self.set_last_index(target, 0.0, instruction)?;
        }
        // Every match is collected *before* any replacement is built, because a replacer
        // function is allowed to move `lastIndex` and the specification does not let it change
        // which matches the replacement covers.
        let mut results = Vec::new();
        loop {
            let result = self.regexp_exec_abstract_value(
                target,
                Value::String(subject.clone()),
                instruction,
            )?;
            if matches!(result, Value::Null) {
                break;
            }
            let matched_value = self.read_property(&result, "0", instruction)?;
            let matched = self.as_text_units(&matched_value, instruction)?;
            results.push(result);
            if !global {
                break;
            }
            if matched.is_empty() {
                let index = self.read_length(target, "lastIndex", instruction)?;
                let next = advance_string_index(&units, index, full_unicode);
                self.set_last_index(target, next as f64, instruction)?;
            }
        }
        let mut built = Vec::new();
        let mut copied = 0usize;
        for result in results {
            let count = self.read_length(&result, "length", instruction)?.max(1) - 1;
            let matched_value = self.read_property(&result, "0", instruction)?;
            let matched = self.as_text_units(&matched_value, instruction)?;
            let matched_units = matched.to_units();
            let match_units = matched_units.len();
            let index = self.read_property(&result, "index", instruction)?;
            let index = self.as_number_argument(Some(&index), instruction)?;
            let position = if index.is_nan() {
                0
            } else {
                index.trunc().clamp(0.0, units.len() as f64) as usize
            };
            let mut captures = Vec::with_capacity(count);
            for slot in 1..=count {
                let capture = self.read_property(&result, &slot.to_string(), instruction)?;
                captures.push(match capture {
                    Value::Undefined => Value::Undefined,
                    other => Value::String(self.as_text_units(&other, instruction)?),
                });
            }
            let named = self.read_property(&result, "groups", instruction)?;
            let produced = if functional {
                let mut call_arguments = Vec::with_capacity(count + 4);
                call_arguments.push(Value::String(matched.clone()));
                call_arguments.extend(captures.iter().cloned());
                call_arguments.push(Value::Number(position as f64));
                call_arguments.push(Value::String(subject.clone()));
                // The groups object is passed only when there is one, which is what keeps the
                // replacer's argument count stable for a pattern with no named groups.
                if !matches!(named, Value::Undefined) {
                    call_arguments.push(named.clone());
                }
                let value =
                    self.call_value(replacement, Value::Undefined, call_arguments, instruction)?;
                self.as_text_units(&value, instruction)?
            } else {
                StringUnits::from_units(&self.substitute(
                    &template.to_units(),
                    &matched_units,
                    &units,
                    position,
                    &captures,
                    &named,
                    instruction,
                )?)
            };
            // A match that starts before what has already been copied is skipped outright: an
            // `exec` of the program's own may hand back overlapping or backwards positions, and
            // the specification refuses to rewind rather than producing text twice.
            if position >= copied {
                push_bounded_units(&mut built, &units[copied..position])?;
                push_bounded_units(&mut built, &produced.to_units())?;
                copied = (position + match_units).min(units.len());
            }
        }
        push_bounded_units(&mut built, &units[copied..units.len()])?;
        Ok(Value::String(StringUnits::from_units(&built)))
    }

    /// `GetSubstitution`: expand `$&`, `$1`, `` $` ``, `$'` and `$<name>` in a replacement.
    ///
    /// Separate from [`expand_replacement`], which works from the ranges the built-in matcher
    /// produced. Here the captures came out of a result object the program may have built
    /// itself, so there are no ranges -- only the strings it handed back.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn substitute(
        &mut self,
        template: &[u16],
        matched: &[u16],
        units: &[u16],
        position: usize,
        captures: &[Value],
        named: &Value,
        instruction: usize,
    ) -> Result<Vec<u16>, ExecutionError> {
        let tail = (position + matched.len()).min(units.len());
        let mut out = Vec::new();
        let mut at = 0;
        while at < template.len() {
            if template[at] != b'$' as u16 || at + 1 >= template.len() {
                push_bounded_units(&mut out, &template[at..at + 1])?;
                at += 1;
                continue;
            }
            match template[at + 1] {
                unit if unit == b'$' as u16 => {
                    push_bounded_units(&mut out, &[b'$' as u16])?;
                    at += 2;
                }
                unit if unit == b'&' as u16 => {
                    push_bounded_units(&mut out, matched)?;
                    at += 2;
                }
                unit if unit == b'`' as u16 => {
                    push_bounded_units(&mut out, &units[0..position])?;
                    at += 2;
                }
                unit if unit == b'\'' as u16 => {
                    push_bounded_units(&mut out, &units[tail..])?;
                    at += 2;
                }
                unit if unit == b'<' as u16 => {
                    // A pattern with no named groups leaves `$<` alone rather than treating it
                    // as an unterminated group reference: the sequence is only special when
                    // there is a groups object for it to name something in.
                    let Some(end) = template[at + 2..]
                        .iter()
                        .position(|found| *found == b'>' as u16)
                    else {
                        push_bounded_units(&mut out, &[b'$' as u16])?;
                        at += 1;
                        continue;
                    };
                    if matches!(named, Value::Undefined) {
                        push_bounded_units(&mut out, &[b'$' as u16])?;
                        at += 1;
                        continue;
                    }
                    let name: String = String::from_utf16_lossy(&template[at + 2..at + 2 + end]);
                    let value = self.read_property(named, &name, instruction)?;
                    if !matches!(value, Value::Undefined) {
                        push_bounded_units(
                            &mut out,
                            &self.as_text_units(&value, instruction)?.to_units(),
                        )?;
                    }
                    at += end + 3;
                }
                first if (b'0' as u16..=b'9' as u16).contains(&first) => {
                    // Two digits are preferred to one, but only when the two-digit group
                    // exists: `$12` in a pattern with three groups is group one followed by a
                    // literal `2`.
                    let second = template
                        .get(at + 2)
                        .copied()
                        .filter(|digit| (b'0' as u16..=b'9' as u16).contains(digit));
                    let single = first as usize - b'0' as usize;
                    let double = second.map(|digit| single * 10 + (digit as usize - b'0' as usize));
                    let (index, width) = match double {
                        Some(both) if both >= 1 && both <= captures.len() => (both, 3),
                        _ if single >= 1 && single <= captures.len() => (single, 2),
                        _ => {
                            push_bounded_units(&mut out, &[b'$' as u16])?;
                            at += 1;
                            continue;
                        }
                    };
                    if let Value::String(text) = &captures[index - 1] {
                        push_bounded_units(&mut out, &text.to_units())?;
                    }
                    at += width;
                }
                _ => {
                    push_bounded_units(&mut out, &[b'$' as u16])?;
                    at += 1;
                }
            }
        }
        Ok(out)
    }

    /// `RegExp.prototype[@@split]`.
    pub(super) fn regexp_split_symbol(
        &mut self,
        target: &Value,
        subject: &str,
        limit: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let flags = self.read_text(target, "flags", instruction)?;
        let unicode_matching = flags.contains('u') || flags.contains('v');
        // The splitter is sticky whatever the receiver was: `split` walks the subject position
        // by position and needs each attempt to either match *here* or fail, where a
        // non-sticky pattern would skip ahead and lose the piece in between.
        let new_flags = if flags.contains('y') {
            flags.clone()
        } else {
            format!("{flags}y")
        };
        let splitter = self.regexp_species_construct(target, &new_flags, instruction)?;
        let cap = match limit {
            Value::Undefined => u32::MAX as usize,
            other => {
                let number = self.as_number_argument(Some(other), instruction)?;
                to_uint32(number) as usize
            }
        };
        let units: Vec<u16> = subject.encode_utf16().collect();
        let mut parts: Vec<Value> = Vec::new();
        if cap == 0 {
            return Ok(Value::Array(ArrayData::new(parts)));
        }
        // An empty subject has no interior to cut: either the separator matches it, and the
        // result is nothing at all, or it does not, and the result is the subject whole.
        if units.is_empty() {
            let result = self.regexp_exec_abstract(&splitter, subject, instruction)?;
            if !matches!(result, Value::Null) {
                return Ok(Value::Array(ArrayData::new(parts)));
            }
            parts.push(Value::String(subject.to_owned().into()));
            return Ok(Value::Array(ArrayData::new(parts)));
        }
        let mut piece_start = 0usize;
        let mut at = 0usize;
        while at < units.len() {
            self.set_last_index(&splitter, at as f64, instruction)?;
            let result = self.regexp_exec_abstract(&splitter, subject, instruction)?;
            if matches!(result, Value::Null) {
                at = advance_string_index(&units, at, unicode_matching);
                continue;
            }
            let end = self
                .read_length(&splitter, "lastIndex", instruction)?
                .min(units.len());
            // A separator that consumed nothing since the last piece began would cut the same
            // place forever; the walk moves on instead of emitting an empty piece per position.
            if end == piece_start {
                at = advance_string_index(&units, at, unicode_matching);
                continue;
            }
            parts.push(Value::String(utf16_slice(&units, piece_start..at).into()));
            if parts.len() == cap {
                return Ok(Value::Array(ArrayData::new(parts)));
            }
            piece_start = end;
            // A capturing separator contributes its groups to the result, which is what makes
            // `"a1b".split(/(\d)/)` three elements rather than two.
            let count = self.read_length(&result, "length", instruction)?.max(1) - 1;
            for slot in 1..=count {
                let capture = self.read_property(&result, &slot.to_string(), instruction)?;
                parts.push(capture);
                if parts.len() == cap {
                    return Ok(Value::Array(ArrayData::new(parts)));
                }
            }
            at = piece_start;
        }
        parts.push(Value::String(
            utf16_slice(&units, piece_start..units.len()).into(),
        ));
        Ok(Value::Array(ArrayData::new(parts)))
    }

    pub(super) fn replace_with_pattern(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        native: Native,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::String(text) = receiver else {
            return Ok(Value::Undefined);
        };
        let subject: Vec<u16> = text.units().collect();
        let replacement = arguments.get(1).cloned().unwrap_or(Value::Undefined);
        let callable = matches!(
            replacement,
            Value::Function(_) | Value::Native(_, _) | Value::Callback(_, _)
        );

        // A string pattern replaces the first occurrence, or every one for `replaceAll`; a
        // regexp replaces every one only when it carries `g`.
        let (matches, from_regexp) = match arguments.first() {
            Some(Value::RegExp(regexp)) => {
                let all = regexp.global;
                let found = if all {
                    regexp_search_all(regexp, &subject)
                } else {
                    regexp_search(regexp, &subject, 0).into_iter().collect()
                };
                (found, true)
            }
            other => {
                let needle = other
                    .map(value_to_string_units)
                    .transpose()?
                    .unwrap_or_default();
                (
                    literal_matches(
                        &subject,
                        &needle,
                        matches!(native, Native::StringReplaceAll),
                    ),
                    false,
                )
            }
        };
        if from_regexp && let Some((_, groups, _)) = matches.last() {
            self.publish_regexp_legacy_captures(&subject, groups);
        }

        let mut out = Vec::new();
        let mut copied = 0;
        for (range, groups, names) in matches {
            push_bounded_units(&mut out, &subject[copied..range.start])?;
            let matched = StringUnits::from_units(&subject[range.clone()]);
            if callable {
                // The handler receives the match, then each group, then where it was found and
                // the whole subject -- the same arguments the specification passes.
                let mut call_arguments = vec![Value::String(matched.clone())];
                for group in groups.iter().skip(1) {
                    call_arguments.push(match group {
                        Some(range) => {
                            Value::String(StringUnits::from_units(&subject[range.clone()]))
                        }
                        None => Value::Undefined,
                    });
                }
                call_arguments.push(Value::Number(range.start as f64));
                call_arguments.push(Value::String(text.clone()));
                // The groups object is last, and only for a pattern that has names -- which is
                // what keeps the replacer's argument count what it has always been otherwise.
                if !names.is_empty() {
                    call_arguments.push(named_group_object(&names, |group| match group {
                        Some(range) => {
                            Value::String(StringUnits::from_units(&subject[range.clone()]))
                        }
                        None => Value::Undefined,
                    }));
                }
                let produced =
                    self.call_value(&replacement, Value::Undefined, call_arguments, instruction)?;
                push_bounded_units(&mut out, &value_to_string_units(&produced)?)?;
            } else {
                let template = value_to_string_units(&replacement)?;
                let expanded = expand_replacement(&template, &subject, &range, &groups, &names)?;
                push_bounded_units(&mut out, &expanded)?;
            }
            copied = range.end;
        }
        push_bounded_units(&mut out, &subject[copied..subject.len()])?;
        Ok(Value::String(StringUnits::from_units(&out)))
    }

    /// The pattern argument of a string method, promoted to a regexp when it is a plain string.
    ///
    /// `"a.b".split(".")` splits on a literal dot while `"a.b".split(/./)` splits on anything, so
    /// a string argument is escaped rather than compiled as a pattern.
    pub(super) fn pattern_argument(
        &mut self,
        arguments: &[Value],
        flags: &str,
        instruction: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        match arguments.first() {
            Some(value @ Value::RegExp(_)) => Ok(Some(value.clone())),
            // `"abc".match()` is `RegExpCreate(undefined, undefined)`: the empty pattern, which
            // matches at position zero. Answering `null` instead skipped the match entirely.
            None | Some(Value::Undefined) => match build_regexp(&StringUnits::default(), flags) {
                Some(regexp) => Ok(Some(regexp)),
                None => Ok(None),
            },
            Some(value) => {
                // `ToString`, which for an object *calls* its `toString` -- the display spelling
                // is the machine's, and using it here matched the literal text
                // "[object Object]" against the subject rather than what the program said.
                let text = self.as_text(value, instruction)?;
                let escaped = regress::escape(&text);
                match build_regexp(&StringUnits::from(escaped), flags) {
                    Some(regexp) => Ok(Some(regexp)),
                    None => {
                        self.throw_error(
                            ErrorKind::Syntax,
                            "Invalid regular expression".to_owned(),
                            instruction,
                        )?;
                        Ok(None)
                    }
                }
            }
        }
    }

    /// One accessor on `RegExp.prototype`.
    ///
    /// The receiver rule is the specification's and it is not a formality: a regexp answers, the
    /// prototype itself answers `undefined` -- it is an ordinary object with no pattern, and the
    /// specification carves it out so that feature-detecting a flag does not throw -- and
    /// anything else is a `TypeError` rather than a plausible `false`.
    pub(super) fn regexp_accessor(
        &mut self,
        accessor: RegExpAccessor,
        receiver: &Value,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        // `flags` is generic: it spells out whatever the other eight answer *for this receiver*,
        // which is what lets a program build a flag string from a plain object.
        if matches!(accessor, RegExpAccessor::Flags) {
            if !is_reflect_target(receiver) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "RegExp.prototype.flags getter called on a non-object".to_owned(),
                });
            }
            let mut flags = String::new();
            for flag in RegExpAccessor::FLAGS {
                let present = self.read_property(receiver, flag.property_key(), instruction)?;
                if present.is_truthy() {
                    flags.push(
                        flag.letter()
                            .unwrap_or_else(|| unreachable!("every flag accessor has a letter")),
                    );
                }
            }
            return Ok(Value::String(flags.into()));
        }
        let Value::RegExp(regexp) = receiver else {
            // The prototype is the one non-regexp the specification lets through, because it is
            // where a program looks when it feature-detects a flag.
            if matches!(
                receiver,
                Value::Native(Native::PrototypeOf(PrototypeTable::RegExp), _)
            ) {
                return Ok(match accessor {
                    RegExpAccessor::Source => Value::String("(?:)".to_owned().into()),
                    _ => Value::Undefined,
                });
            }
            return Err(ExecutionError::TypeErrorMessage {
                message: format!(
                    "RegExp.prototype.{} getter called on a non-regexp",
                    accessor.property_key()
                ),
            });
        };
        Ok(match accessor {
            RegExpAccessor::Source => Value::String(escaped_source_units(&regexp.source)),
            RegExpAccessor::Flags => unreachable!("handled above"),
            other => Value::Boolean(
                other
                    .letter()
                    .is_some_and(|letter| regexp.flags.contains(letter)),
            ),
        })
    }
}
