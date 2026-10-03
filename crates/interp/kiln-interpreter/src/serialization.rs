use super::*;

/// Build an `Error` instance.
///
/// Frame metadata is retained in an internal slot and formatted lazily by errors.ts. The object
/// builder therefore installs no own `stack` property, allowing the shared accessor to cache the
/// formatted string on first read without exposing raw frame records to reflection.
///
/// `cause` follows InstallErrorCause: the property exists when the options object *has* a
/// `cause` key, including when its value is `undefined`. Testing presence rather than value is
/// the whole point of that step in the specification.
///
/// `name` is deliberately **not** an own property. It lives on `Error.prototype` and on each
/// kind's prototype, exactly as the specification puts it, and an own one here would shadow
/// anything a subclass says about itself: a `name = "TimeoutError"` class field is installed on
/// the instance during construction, and an own `name` written afterwards overwrote it with
/// `"Error"`. `catch (e) { if (e.name === "TimeoutError") }` -- how a server tells a timeout
/// from a real failure -- then took the wrong branch, and so did a `get name()` accessor on the
/// subclass prototype, which an own property on the instance shadows outright.
///
/// `message` is own only when one was passed, which is the other half of the same rule: the
/// empty default belongs to the prototype too.
pub(super) fn build_error(kind: ErrorKind, arguments: &[Value]) -> Value {
    let message_at = kind.message_index();
    let given = match arguments.get(message_at) {
        None | Some(Value::Undefined) => None,
        Some(value) => Some(value.to_display_string()),
    };
    let mut data = ObjectData {
        error_brand: true,
        ..ObjectData::default()
    };
    // Every own property of an error is non-enumerable, which is why `JSON.stringify(err)` is
    // `{}` and `Object.keys(err)` is empty -- an error is meant to be *read*, not walked, and a
    // program that serialises one is asking for the fields it names rather than all of them.
    //
    if let Some(message) = given {
        data.set_non_enumerable("message", Value::String(message.into()));
    }
    for (index, field) in kind.carried_fields().iter().enumerate() {
        data.set_non_enumerable(
            field,
            arguments.get(index).cloned().unwrap_or(Value::Undefined),
        );
    }
    if let Some(Value::Object(options)) = arguments.get(message_at + 1)
        && let Some(cause) = options.borrow().get("cause")
    {
        data.set_non_enumerable("cause", cause.clone());
    }
    Value::Object(Rc::new(RefCell::new(data)))
}

/// The `space` argument of `JSON.stringify`, resolved to the literal indent for one level.
///
/// A number is that many spaces and a string is used as-is, both capped at ten UTF-16 units,
/// which is what the specification says. Keeping the result as units matters for an indent that
/// contains an unpaired surrogate: converting it through a scalar string would replace the unit
/// before the serializer had a chance to emit it.
pub(super) fn json_indent(space: Option<&Value>) -> StringUnits {
    const MAX: usize = 10;
    // `JSON.stringify(v, null, new Number(2))` indents by two: the specification unwraps a
    // Number or String object before looking at it.
    let unwrapped = space.and_then(wrapped_primitive);
    let space = unwrapped.as_ref().or(space);
    match space {
        Some(Value::Number(count)) if *count >= 1.0 => {
            StringUnits::from(" ".repeat((*count as usize).min(MAX)))
        }
        Some(Value::String(text)) => {
            let units = text.units().take(MAX).collect::<Vec<_>>();
            StringUnits::from_units(&units)
        }
        _ => StringUnits::default(),
    }
}

/// A checked builder for language strings.
///
/// JSON output is a JavaScript string. Keep the common Latin1 representation until a wide unit
/// appears, then promote once; the checked budget follows the actual backing storage (one byte
/// per Latin1 unit or two bytes per UTF-16 unit), rather than routing output through scalar text.
pub(super) enum StringUnitsStorage {
    Latin1(Vec<u8>),
    Utf16(Vec<u16>),
}

pub(super) struct StringUnitsBuilder {
    pub(super) storage: StringUnitsStorage,
    pub(super) max_bytes: usize,
}

impl StringUnitsBuilder {
    pub(super) fn new() -> Self {
        Self::with_limit(MAX_STRING_STORAGE_BYTES)
    }

    pub(super) fn with_limit(max_bytes: usize) -> Self {
        Self {
            storage: StringUnitsStorage::Latin1(Vec::new()),
            max_bytes,
        }
    }

    pub(super) fn storage_len(&self) -> usize {
        match &self.storage {
            StringUnitsStorage::Latin1(bytes) => bytes.len(),
            StringUnitsStorage::Utf16(units) => units.len(),
        }
    }

    pub(super) fn check_capacity(
        &self,
        additional_units: usize,
        wide: bool,
    ) -> Result<(), ExecutionError> {
        let units = self
            .storage_len()
            .checked_add(additional_units)
            .ok_or(ExecutionError::StringTooLong { bytes: usize::MAX })?;
        let bytes = units
            .checked_mul(if wide { 2 } else { 1 })
            .ok_or(ExecutionError::StringTooLong { bytes: usize::MAX })?;
        if bytes > self.max_bytes {
            return Err(ExecutionError::StringTooLong { bytes });
        }
        Ok(())
    }

    pub(super) fn push_latin1(&mut self, bytes: &[u8]) -> Result<(), ExecutionError> {
        let wide = matches!(self.storage, StringUnitsStorage::Utf16(_));
        self.check_capacity(bytes.len(), wide)?;
        match &mut self.storage {
            StringUnitsStorage::Latin1(storage) => {
                storage.extend_from_slice(bytes);
            }
            StringUnitsStorage::Utf16(storage) => {
                storage.extend(bytes.iter().copied().map(u16::from));
            }
        }
        Ok(())
    }

    pub(super) fn push_ascii(&mut self, text: &str) -> Result<(), ExecutionError> {
        debug_assert!(text.is_ascii());
        self.push_latin1(text.as_bytes())
    }

    pub(super) fn push_units(&mut self, text: &StringUnits) -> Result<(), ExecutionError> {
        match text.as_view() {
            StringView::Latin1(bytes) => self.push_latin1(bytes),
            StringView::Utf16(units) => self.push_utf16(units),
        }
    }

    pub(super) fn push_utf16(&mut self, units: &[u16]) -> Result<(), ExecutionError> {
        if matches!(self.storage, StringUnitsStorage::Utf16(_)) {
            self.check_capacity(units.len(), true)?;
            if let StringUnitsStorage::Utf16(old) = &mut self.storage {
                old.extend_from_slice(units);
            }
            return Ok(());
        }
        if units.iter().all(|&unit| unit <= 0xff) {
            self.check_capacity(units.len(), false)?;
            if let StringUnitsStorage::Latin1(old) = &mut self.storage {
                old.extend(units.iter().copied().map(|unit| unit as u8));
            }
            return Ok(());
        }
        // Promotion doubles the storage occupied by the existing Latin1 prefix. Check the
        // complete final representation before mutating it, so a refusal cannot leave a partial
        // builder behind.
        self.check_capacity(units.len(), true)?;
        match &mut self.storage {
            StringUnitsStorage::Latin1(old) => {
                let mut wide = Vec::with_capacity(old.len() + units.len());
                wide.extend(old.iter().copied().map(u16::from));
                wide.extend_from_slice(units);
                self.storage = StringUnitsStorage::Utf16(wide);
            }
            StringUnitsStorage::Utf16(old) => old.extend_from_slice(units),
        }
        Ok(())
    }

    pub(super) fn finish(self) -> StringUnits {
        match self.storage {
            StringUnitsStorage::Latin1(bytes) => StringUnits::from_view(StringView::Latin1(&bytes)),
            StringUnitsStorage::Utf16(units) => StringUnits::from_view(StringView::Utf16(&units)),
        }
    }
}

pub(super) fn units_concat(
    left: &StringUnits,
    right: &StringUnits,
) -> Result<StringUnits, ExecutionError> {
    let mut out = StringUnitsBuilder::new();
    out.push_units(left)?;
    out.push_units(right)?;
    Ok(out.finish())
}

/// The three points at which serialising JSON calls back into the program.
///
/// One callback rather than three, because each needs the machine and two closures cannot hold
/// `&mut self` at once -- the compiler says so, and the alternative of passing the interpreter
/// itself would make the serialiser a method on it.
pub(super) enum JsonCall<'a> {
    /// An accessor property: reading it is a call.
    Getter(&'a Value, &'a Value),
    /// `toJSON`, which runs before anything else is decided about a value, and which the
    /// specification hands the key the value is being serialised under.
    ToJson(&'a Value, &'a StringUnits),
    /// The `replacer` function, as `(holder, key, value)`.
    Replace(&'a Value, &'a StringUnits, Value),
}

pub(super) type JsonResolver<'a> = &'a mut dyn FnMut(JsonCall<'_>) -> Result<Value, ExecutionError>;

/// Serialise one value, calling back into the interpreter where the specification says to.
///
/// `None` is "not representable" -- `undefined`, a function, a symbol. The distinction matters
/// because the caller decides what to do with it: an object property is dropped, an array
/// element becomes `null`, and at the top level the whole call returns `undefined`.
#[allow(clippy::too_many_arguments)]
pub(super) fn json_stringify(
    value: &Value,
    indent: &StringUnits,
    current: &StringUnits,
    seen: &mut Vec<usize>,
    resolve: JsonResolver<'_>,
    allowed: Option<&[StringUnits]>,
    key: &StringUnits,
    holder: &Value,
) -> Result<Option<StringUnits>, ExecutionError> {
    // `toJSON` runs before anything else is decided about the value, which is how a `Date`
    // serialises as its ISO string and how a library controls its own JSON form.
    let replaced = resolve(JsonCall::ToJson(value, key))?;
    // Then the replacer, in that order, on every value including the root.
    let replaced = resolve(JsonCall::Replace(holder, key, replaced))?;
    let value = &replaced;
    let nested = units_concat(current, indent)?;
    let pretty = !indent.is_empty();

    Ok(match value {
        // Omitted rather than serialised: JSON has no way to spell any of these. An array
        // element that is omitted becomes null, which is also what a hole serialises as.
        Value::Native(Native::PrototypeOf(table), _) if *table != PrototypeTable::Function => {
            Some(StringUnits::from("{}"))
        }
        Value::Undefined
        | Value::Function(_)
        | Value::Native(_, _)
        | Value::Callback(_, _)
        | Value::Element(_)
        | Value::CommonJsRequire(_)
        | Value::CommonJsResolve(_) => None,
        Value::Symbol(_) => None,
        Value::Null => Some(StringUnits::from("null")),
        Value::Boolean(value) => Some(StringUnits::from(value.to_string())),
        Value::Number(number) if number.is_finite() => {
            Some(StringUnits::from(number_string(*number)))
        }
        Value::Number(_) => Some(StringUnits::from("null")),
        Value::String(text) => Some(json_quote(text)?),
        Value::Array(items) => {
            let guard = CycleGuard::enter(seen, Rc::as_ptr(items).cast::<()>())?;
            // An accessor at an index leaves a hole in dense storage, and serialising it as null
            // would report as absent an element the program reads perfectly well.
            let items = if items.elements.borrow().is_empty() {
                items.borrow().clone()
            } else {
                let count = items.borrow().len();
                let mut collected = Vec::with_capacity(count);
                for index in 0..count {
                    let element = match array_exotic_element(items, index) {
                        Some(ExoticElement {
                            property: Property::Accessor { get, .. },
                            ..
                        }) => match &get {
                            Some(getter) => resolve(JsonCall::Getter(getter, value))?,
                            None => Value::Undefined,
                        },
                        _ => items
                            .borrow()
                            .get(index)
                            .cloned()
                            .unwrap_or(Value::Undefined),
                    };
                    collected.push(element);
                }
                collected
            };
            let mut out = StringUnitsBuilder::new();
            out.push_ascii("[")?;
            if !items.is_empty() {
                if pretty {
                    out.push_ascii("\n")?;
                    out.push_units(&nested)?;
                }
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        if pretty {
                            out.push_ascii(",\n")?;
                            out.push_units(&nested)?;
                        } else {
                            out.push_ascii(",")?;
                        }
                    }
                    let at = StringUnits::from(index.to_string());
                    let part = json_stringify(
                        item, indent, &nested, guard.seen, resolve, allowed, &at, value,
                    )?;
                    out.push_units(&part.unwrap_or_else(|| StringUnits::from("null")))?;
                }
                if pretty {
                    out.push_ascii("\n")?;
                    out.push_units(current)?;
                }
            }
            out.push_ascii("]")?;
            Some(out.finish())
        }
        // A primitive wrapper is serialised as its primitive value, not as an empty object.
        Value::Object(object)
            if object.borrow().primitive().is_some() && object.borrow().raw_json_text.is_none() =>
        {
            let inner = object.borrow().primitive().cloned();
            return match inner {
                Some(inner) => {
                    json_stringify(&inner, indent, current, seen, resolve, allowed, key, holder)
                }
                None => Ok(None),
            };
        }
        // Raw JSON has already been validated. Its units must be emitted verbatim, including
        // unpaired surrogates, because preserving the source text is the purpose of rawJSON.
        Value::Object(object) if object.borrow().raw_json_text.is_some() => {
            let raw = object.borrow().raw_json_text.clone().unwrap_or_default();
            Some(raw)
        }
        Value::Object(object) => {
            let guard = CycleGuard::enter(seen, Rc::as_ptr(object).cast::<()>())?;
            // Read out before iterating, so the object is not borrowed while a getter runs.
            let entries = own_string_properties_units(&Intrinsics::default(), value);
            let entries: Vec<(StringUnits, Property)> = match allowed {
                Some(keys) => keys
                    .iter()
                    .filter_map(|name| {
                        entries
                            .iter()
                            .find(|(key, _)| key == name)
                            .map(|(key, property)| (key.clone(), property.clone()))
                    })
                    .collect(),
                None => entries,
            };
            let mut out = StringUnitsBuilder::new();
            out.push_ascii("{")?;
            let mut emitted = 0usize;
            for (key, property) in entries {
                let property = match property {
                    Property::Value(value) => value,
                    Property::Accessor { get, .. } => match get {
                        Some(getter) => resolve(JsonCall::Getter(&getter, value))?,
                        None => continue,
                    },
                };
                let Some(part) = json_stringify(
                    &property, indent, &nested, guard.seen, resolve, allowed, &key, value,
                )?
                else {
                    continue;
                };
                if emitted > 0 {
                    if pretty {
                        out.push_ascii(",\n")?;
                        out.push_units(&nested)?;
                    } else {
                        out.push_ascii(",")?;
                    }
                } else if pretty {
                    out.push_ascii("\n")?;
                    out.push_units(&nested)?;
                }
                out.push_units(&json_quote(&key)?)?;
                out.push_ascii(if pretty { ": " } else { ":" })?;
                out.push_units(&part)?;
                emitted += 1;
            }
            if emitted > 0 && pretty {
                out.push_ascii("\n")?;
                out.push_units(current)?;
            }
            out.push_ascii("}")?;
            Some(out.finish())
        }
        // None of these has own enumerable properties, so all serialise as the empty object.
        Value::Generator(_)
        | Value::Promise(_)
        | Value::RegExp(_)
        | Value::Temporal(_)
        | Value::Intl(_) => Some(StringUnits::from("{}")),
        Value::Date(date) => Some(match iso_string(date.time.get()) {
            Some(text) => json_quote(&StringUnits::from(text))?,
            None => StringUnits::from("null"),
        }),
        Value::Settler(..) | Value::Capability(..) | Value::Bound(_) | Value::Wrapped(_) => None,
        Value::Proxy(_) => return Err(ExecutionError::ProxyNotSerialisable),
        // Typed arrays serialise as objects keyed by index.
        Value::TypedArray(view) => {
            let entries: Vec<(StringUnits, Value)> = view
                .items()
                .into_iter()
                .enumerate()
                .map(|(index, item)| (StringUnits::from(index.to_string()), item))
                .collect();
            let mut data = ObjectData::default();
            for (key, item) in entries {
                data.set_units(&key, item);
            }
            return json_stringify(
                &Value::Object(Rc::new(RefCell::new(data))),
                indent,
                current,
                seen,
                resolve,
                allowed,
                key,
                holder,
            );
        }
        Value::ArrayBuffer(_) | Value::DataView(_) => Some(StringUnits::from("{}")),
        Value::Shared(facade) if facade.is_array() => {
            let guard = CycleGuard::enter(seen, Rc::as_ptr(facade).cast::<()>())?;
            let length = facade
                .array_length()
                .expect("shared array kind was checked");
            let mut out = StringUnitsBuilder::new();
            out.push_ascii("[")?;
            if length > 0 && pretty {
                out.push_ascii("\n")?;
                out.push_units(&nested)?;
            }
            for index in 0..length {
                if index > 0 {
                    out.push_ascii(if pretty { ",\n" } else { "," })?;
                    if pretty {
                        out.push_units(&nested)?;
                    }
                }
                let at = StringUnits::from(index.to_string());
                let item = facade.own_property(PropertyKeyRef::Units(&at)).map_or(
                    Value::Undefined,
                    |(property, _)| match property {
                        Property::Value(value) => value,
                        Property::Accessor { .. } => unreachable!("sealed properties are data"),
                    },
                );
                let part = json_stringify(
                    &item, indent, &nested, guard.seen, resolve, allowed, &at, value,
                )?;
                out.push_units(&part.unwrap_or_else(|| StringUnits::from("null")))?;
            }
            if length > 0 && pretty {
                out.push_ascii("\n")?;
                out.push_units(current)?;
            }
            out.push_ascii("]")?;
            Some(out.finish())
        }
        Value::Shared(facade) => {
            let guard = CycleGuard::enter(seen, Rc::as_ptr(facade).cast::<()>())?;
            let entries = facade.own_fields();
            let entries: Vec<_> = match allowed {
                Some(keys) => keys
                    .iter()
                    .filter_map(|name| entries.iter().find(|(key, _, _)| key == name).cloned())
                    .collect(),
                None => entries,
            };
            let mut out = StringUnitsBuilder::new();
            out.push_ascii("{")?;
            let mut emitted = 0usize;
            for (key, property, attributes) in entries {
                if !attributes.enumerable {
                    continue;
                }
                let Property::Value(item) = property else {
                    unreachable!("sealed properties are data");
                };
                let Some(part) = json_stringify(
                    &item, indent, &nested, guard.seen, resolve, allowed, &key, value,
                )?
                else {
                    continue;
                };
                if emitted > 0 {
                    out.push_ascii(if pretty { ",\n" } else { "," })?;
                    if pretty {
                        out.push_units(&nested)?;
                    }
                } else if pretty {
                    out.push_ascii("\n")?;
                    out.push_units(&nested)?;
                }
                out.push_units(&json_quote(&key)?)?;
                out.push_ascii(if pretty { ": " } else { ":" })?;
                out.push_units(&part)?;
                emitted += 1;
            }
            if emitted > 0 && pretty {
                out.push_ascii("\n")?;
                out.push_units(current)?;
            }
            out.push_ascii("}")?;
            Some(out.finish())
        }
        Value::BigInt(_) => return Err(ExecutionError::BigIntNotSerialisable),
        Value::Cell(_) => unreachable!("a cell is never used as a value"),
    })
}

/// A JSON string literal, with the escapes the grammar requires.
///
/// Ordinary strings use the well-formed JSON.stringify rule: an unpaired surrogate is escaped
/// as `\\uXXXX`, while a valid surrogate pair remains two original units. This preserves every
/// JavaScript string value without routing it through a lossy scalar conversion.
pub(super) fn json_quote(text: &StringUnits) -> Result<StringUnits, ExecutionError> {
    let mut out = StringUnitsBuilder::new();
    out.push_ascii("\"")?;
    match text.as_view() {
        StringView::Latin1(bytes) => quote_latin1(&mut out, bytes)?,
        StringView::Utf16(units) => quote_utf16(&mut out, units)?,
    }
    out.push_ascii("\"")?;
    Ok(out.finish())
}

pub(super) fn push_json_unicode_escape(
    out: &mut StringUnitsBuilder,
    unit: u16,
) -> Result<(), ExecutionError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = [
        b'\\',
        b'u',
        HEX[((unit >> 12) & 0xf) as usize],
        HEX[((unit >> 8) & 0xf) as usize],
        HEX[((unit >> 4) & 0xf) as usize],
        HEX[(unit & 0xf) as usize],
    ];
    out.push_latin1(&bytes)
}

pub(super) fn quote_latin1(
    out: &mut StringUnitsBuilder,
    bytes: &[u8],
) -> Result<(), ExecutionError> {
    let mut start = 0;
    for (index, &byte) in bytes.iter().enumerate() {
        let escape = match byte {
            b'"' => Some(b"\\\"".as_slice()),
            b'\\' => Some(b"\\\\".as_slice()),
            b'\n' => Some(b"\\n".as_slice()),
            b'\r' => Some(b"\\r".as_slice()),
            b'\t' => Some(b"\\t".as_slice()),
            0x08 => Some(b"\\b".as_slice()),
            0x0c => Some(b"\\f".as_slice()),
            byte if byte < 0x20 => None,
            _ => continue,
        };
        out.push_latin1(&bytes[start..index])?;
        if let Some(escape) = escape {
            out.push_latin1(escape)?;
        } else {
            push_json_unicode_escape(out, u16::from(byte))?;
        }
        start = index + 1;
    }
    out.push_latin1(&bytes[start..])
}

pub(super) fn quote_utf16(
    out: &mut StringUnitsBuilder,
    units: &[u16],
) -> Result<(), ExecutionError> {
    let mut start = 0;
    let mut index = 0;
    while index < units.len() {
        let unit = units[index];
        let raw_pair = (0xd800..=0xdbff).contains(&unit)
            && units
                .get(index + 1)
                .is_some_and(|next| (0xdc00..=0xdfff).contains(next));
        if raw_pair {
            index += 2;
            continue;
        }
        let escape = match unit {
            0x22 => Some(b"\\\"".as_slice()),
            0x5c => Some(b"\\\\".as_slice()),
            0x0a => Some(b"\\n".as_slice()),
            0x0d => Some(b"\\r".as_slice()),
            0x09 => Some(b"\\t".as_slice()),
            0x08 => Some(b"\\b".as_slice()),
            0x0c => Some(b"\\f".as_slice()),
            unit if unit < 0x20 => None,
            0xd800..=0xdfff => None,
            _ => {
                index += 1;
                continue;
            }
        };
        out.push_utf16(&units[start..index])?;
        if let Some(escape) = escape {
            out.push_latin1(escape)?;
        } else {
            push_json_unicode_escape(out, unit)?;
        }
        index += 1;
        start = index;
    }
    out.push_utf16(&units[start..])
}

/// Tracks the containers on the path from the root, so a value containing itself is caught
/// rather than recursed into forever.
///
/// The identity is the allocation address, which is exactly the identity JavaScript uses. Being
/// a guard rather than a bare push means the entry is removed on the error path too, so the set
/// stays the current *path* and a value legitimately reachable twice is not mistaken for a cycle.
pub(super) struct CycleGuard<'a> {
    pub(super) seen: &'a mut Vec<usize>,
}

impl<'a> CycleGuard<'a> {
    pub(super) fn enter(
        seen: &'a mut Vec<usize>,
        container: *const (),
    ) -> Result<Self, ExecutionError> {
        let identity = container as usize;
        if seen.contains(&identity) {
            return Err(ExecutionError::CyclicJson);
        }
        seen.push(identity);
        Ok(Self { seen })
    }
}

impl Drop for CycleGuard<'_> {
    fn drop(&mut self) {
        self.seen.pop();
    }
}
