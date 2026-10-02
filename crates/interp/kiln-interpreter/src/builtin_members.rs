use super::*;

/// Whether a value can be called.
/// Whether the iterator protocol will accept this value where it requires an object.
///
/// Asked of a receiver, of a `next` result and of whatever a `flatMap` mapper answered. Getting
/// it wrong in either direction is a silent bug: a primitive accepted as an iterator reads
/// `done` as `undefined` and then loops forever.
pub(super) fn is_iterator_object(value: &Value) -> bool {
    matches!(
        value,
        Value::Object(_)
            | Value::Array(_)
            | Value::Function(_)
            | Value::Generator(_)
            | Value::Proxy(_)
            | Value::TypedArray(_)
            | Value::DataView(_)
            | Value::ArrayBuffer(_)
            | Value::Date(_)
            | Value::RegExp(_)
            | Value::Promise(_)
            | Value::Bound(_)
            | Value::Native(_, _)
            | Value::Callback(_, _)
    )
}

/// Whether reading or writing `name` on `target` reaches the restricted pair.
///
/// `caller` and `arguments` live on `%Function.prototype%` and nowhere else, so this is exactly
/// "is `target` something that inherits from it" -- which is "is it callable", asked the way the
/// arity table asks it: a namespace like `Math` is a `Native` too and is not a function.
pub(super) fn is_restricted_function_property(target: &Value, name: &str) -> bool {
    matches!(name, "caller" | "arguments") && function_identity(target).is_some()
}

/// What `%ThrowTypeError%` throws, for the paths that reach the pair without calling it.
pub(super) fn restricted_property_error() -> ExecutionError {
    ExecutionError::TypeErrorMessage {
        message: "'caller', 'callee', and 'arguments' properties may not be accessed on strict \
                  mode functions or the arguments objects for calls to them"
            .to_owned(),
    }
}

/// Whether argument `index` of `native` is one the specification converts with `ToString`.
///
/// Spelled out per method because it is not uniform: `concat` converts every argument, `split`
/// converts the separator but not the limit beside it, and `replace` converts the search value
/// but must leave a replacement function alone for the call that follows.
pub(super) fn string_coerced_argument(native: Native, index: usize) -> bool {
    match native {
        Native::StringConcat => true,
        Native::StringSplit
        | Native::StringIndexOf
        | Native::StringLastIndexOf
        | Native::StringIncludes
        | Native::StringStartsWith
        | Native::StringEndsWith => index == 0,
        Native::StringReplace | Native::StringReplaceAll => index <= 1,
        Native::StringPadStart | Native::StringPadEnd => index == 1,
        _ => false,
    }
}

/// Callables whose receiver is ignored or whose own call object already owns the receiver.
///
/// Native and compiled functions are intentionally excluded: their receiver is observable and
/// each dispatch path must preserve its specialized fast path. This match is deliberately
/// exhaustive: adding a `Value` representation forces its call classification to be decided here.
pub(super) fn forwards_to_call_value(value: &Value) -> bool {
    match value {
        Value::Settler(..)
        | Value::Capability(..)
        | Value::Element(_)
        | Value::Bound(_)
        | Value::Wrapped(_)
        | Value::CommonJsRequire(_)
        | Value::CommonJsResolve(_) => true,
        Value::Undefined
        | Value::Null
        | Value::Boolean(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::Cell(_)
        | Value::Native(_, _)
        | Value::Generator(_)
        | Value::Promise(_)
        | Value::RegExp(_)
        | Value::Date(_)
        | Value::Temporal(_)
        | Value::Intl(_)
        | Value::BigInt(_)
        | Value::Array(_)
        | Value::Callback(_, _)
        | Value::TypedArray(_)
        | Value::ArrayBuffer(_)
        | Value::DataView(_)
        | Value::Proxy(_)
        | Value::Symbol(_)
        | Value::Object(_)
        | Value::Shared(_)
        | Value::Function(_) => false,
    }
}

pub(crate) fn is_callable(value: &Value) -> bool {
    match value {
        // A proxy has a `[[Call]]` method exactly when its target has one, decided when the
        // proxy is made and never after. Treating every proxy as callable meant
        // `new Proxy({}, {apply(){}})()` ran the trap where the language says the proxy is not
        // a function at all -- and `typeof` said so too. The target survives revocation, which
        // is what keeps a revoked function proxy a function.
        Value::Proxy(proxy) => is_callable(&proxy.target),
        Value::Native(
            Native::AsyncFunctionPrototype
            | Native::GeneratorFunctionPrototype
            | Native::AsyncGeneratorFunctionPrototype,
            _,
        ) => false,
        other => {
            matches!(
                other,
                Value::Function(_) | Value::Native(_, _) | Value::Callback(_, _)
            ) || forwards_to_call_value(other)
        }
    }
}

/// One match: where it landed, and where each capture group did.
///
/// Every range is in UTF-16 units, which is what the language counts in and what the engine
/// reports when matching over UTF-16.
/// The third element is the pattern's *named* groups, in declaration order and already
/// de-duplicated by `regress`: the duplicate-named-groups proposal lets two alternatives share a
/// name, and only one of them can have participated.
pub(super) type RegExpMatch = (
    std::ops::Range<usize>,
    Vec<Option<std::ops::Range<usize>>>,
    Vec<(String, Option<std::ops::Range<usize>>)>,
);

/// Where a literal needle occurs, in the shape a regexp match has, so replacement is one path.
pub(super) fn literal_matches(subject: &[u16], needle: &[u16], all: bool) -> Vec<RegExpMatch> {
    let mut found = Vec::new();
    let mut at = 0;
    while at + needle.len() <= subject.len() {
        if subject[at..at + needle.len()] == *needle {
            let range = at..at + needle.len();
            // A literal needle has no groups at all, named or otherwise.
            found.push((range.clone(), vec![Some(range)], Vec::new()));
            if !all {
                break;
            }
            // An empty needle would never advance, so it steps one at a time.
            at = if needle.is_empty() {
                at + 1
            } else {
                at + needle.len()
            };
            continue;
        }
        at += 1;
    }
    found
}

/// Expand `$&`, `` $` ``, `$'`, `$1`..`$99` and `$$` in a replacement string.
/// The text one match expands to, with `$&`, `` $` ``, `$'`, `$$` and `$n` resolved.
///
/// Bounded: one `$1` costs the whole match, and a template carrying tens of thousands of them
/// against a megabyte-long match asks for a string in the tens of gigabytes -- from a single
/// call, before the caller ever sees a byte of it.
pub(super) fn expand_replacement(
    template: &str,
    subject: &[u16],
    range: &std::ops::Range<usize>,
    groups: &[Option<std::ops::Range<usize>>],
    names: &[(String, Option<std::ops::Range<usize>>)],
) -> Result<String, ExecutionError> {
    let mut out = String::new();
    let characters: Vec<char> = template.chars().collect();
    let mut at = 0;
    while at < characters.len() {
        if characters[at] != '$' || at + 1 >= characters.len() {
            push_bounded_char(&mut out, characters[at])?;
            at += 1;
            continue;
        }
        match characters[at + 1] {
            '$' => {
                push_bounded_char(&mut out, '$')?;
                at += 2;
            }
            '&' => {
                push_bounded(&mut out, &utf16_slice(subject, range.clone()))?;
                at += 2;
            }
            '`' => {
                push_bounded(&mut out, &utf16_slice(subject, 0..range.start))?;
                at += 2;
            }
            '\'' => {
                push_bounded(&mut out, &utf16_slice(subject, range.end..subject.len()))?;
                at += 2;
            }
            first if first.is_ascii_digit() => {
                // Two digits are preferred when that names a group, which is why `$12` is group
                // twelve where there are twelve and group one followed by "2" where there are not.
                let two = characters
                    .get(at + 2)
                    .filter(|second| second.is_ascii_digit())
                    .and_then(|second| format!("{first}{second}").parse::<usize>().ok())
                    .filter(|index| *index > 0 && *index < groups.len());
                let (index, width) = match two {
                    Some(index) => (index, 3),
                    None => match first.to_digit(10).map(|digit| digit as usize) {
                        Some(index) if index > 0 && index < groups.len() => (index, 2),
                        // Not a group reference at all, so the text stands as written.
                        _ => {
                            push_bounded_char(&mut out, '$')?;
                            at += 1;
                            continue;
                        }
                    },
                };
                if let Some(Some(group)) = groups.get(index) {
                    push_bounded(&mut out, &utf16_slice(subject, group.clone()))?;
                }
                at += width;
            }
            // `$<name>` is a group reference only for a pattern that *has* named groups: in
            // one that does not, the specification leaves the text exactly as written, so
            // `"x".replace(/x/, "$<a>")` is the literal `$<a>` rather than an empty string.
            '<' if !names.is_empty() => {
                let Some(end) = characters[at + 2..].iter().position(|c| *c == '>') else {
                    push_bounded_char(&mut out, '$')?;
                    at += 1;
                    continue;
                };
                let name: String = characters[at + 2..at + 2 + end].iter().collect();
                // A name the pattern does not declare expands to nothing, like a group that did
                // not participate -- it is not an error, and not the literal text either.
                if let Some((_, Some(group))) = names.iter().find(|(key, _)| *key == name) {
                    push_bounded(&mut out, &utf16_slice(subject, group.clone()))?;
                }
                at += 3 + end;
            }
            _ => {
                push_bounded_char(&mut out, '$')?;
                at += 1;
            }
        }
    }
    Ok(out)
}

/// Every match of a global pattern, from the start of the subject.
///
/// An empty match advances the cursor by one, or a pattern that can match nothing -- `/a*/g` --
/// would never finish.
pub(super) fn regexp_search_all(regexp: &RegExpData, subject: &[u16]) -> Vec<RegExpMatch> {
    let mut found = Vec::new();
    let mut at = 0;
    while at <= subject.len() {
        let Some((range, groups, names)) = regexp_search(regexp, subject, at) else {
            break;
        };
        at = if range.end == range.start {
            range.end + 1
        } else {
            range.end
        };
        found.push((range, groups, names));
    }
    found
}

/// A capture range as the two-element array `indices` reports it, or `undefined`.
pub(super) fn indices_pair(group: Option<&std::ops::Range<usize>>) -> Value {
    match group {
        Some(range) => Value::Array(ArrayData::new(vec![
            Value::Number(range.start as f64),
            Value::Number(range.end as f64),
        ])),
        None => Value::Undefined,
    }
}

/// The object `result.groups` and `result.indices.groups` are: one key per *name*, in declaration
/// order, and no prototype.
///
/// No prototype because the specification says `OrdinaryObjectCreate(null)`, and it is visible:
/// a pattern with a group called `toString` would otherwise inherit one, and
/// `result.groups.__proto__` would answer `Object.prototype` where a program checking for a
/// group of that name expects `undefined`.
pub(super) fn named_group_object(
    names: &[(String, Option<std::ops::Range<usize>>)],
    value: impl Fn(Option<&std::ops::Range<usize>>) -> Value,
) -> Value {
    let mut object = ObjectData::default();
    object.set_prototype(Value::Null);
    for (name, range) in names {
        object.set(name, value(range.as_ref()));
    }
    Value::Object(Rc::new(RefCell::new(object)))
}

/// The array `exec` and `match` produce: the whole match, then each group, with `index` and
/// `input` alongside.
pub(super) fn match_result(
    subject: &[u16],
    range: &std::ops::Range<usize>,
    groups: &[Option<std::ops::Range<usize>>],
    names: &[(String, Option<std::ops::Range<usize>>)],
    has_indices: bool,
) -> Value {
    let mut items = vec![Value::String(StringUnits::from_units(
        &subject[range.clone()],
    ))];
    // The first group `regress` reports is the whole match, which is already element zero.
    for group in groups.iter().skip(1) {
        items.push(match group {
            // Sliced as units: the match text keeps lone surrogates rather than replacing
            // them on the way into the result array.
            Some(range) => Value::String(StringUnits::from_units(&subject[range.clone()])),
            // A group that did not participate is undefined rather than empty.
            None => Value::Undefined,
        });
    }
    // `index` and `input` are properties *of the array*, which is why an array needs somewhere
    // to keep a property that is not an element.
    let result = ArrayData::new(items);
    result
        .properties
        .borrow_mut()
        .set("index", Value::Number(range.start as f64));
    result.properties.borrow_mut().set(
        "input",
        Value::String(StringUnits::from_units(&subject[0..subject.len()])),
    );
    // Present and undefined rather than absent when the pattern has no named groups: a program
    // reads `m.groups?.name`, and node shows the key. With names, it is a real object -- reading
    // `m.groups.year` off `undefined` was a TypeError from ordinary, widely written code.
    let named = if names.is_empty() {
        Value::Undefined
    } else {
        named_group_object(names, |group| match group {
            Some(range) => Value::String(StringUnits::from_units(&subject[range.clone()])),
            None => Value::Undefined,
        })
    };
    result.properties.borrow_mut().set("groups", named);
    // `indices` is the `d` flag's whole observable effect: the same shape as the result itself,
    // holding `[start, end]` pairs instead of text, and carrying a `groups` of its own.
    if has_indices {
        let mut pairs = vec![indices_pair(Some(range))];
        for group in groups.iter().skip(1) {
            pairs.push(indices_pair(group.as_ref()));
        }
        let indices = ArrayData::new(pairs);
        let named = if names.is_empty() {
            Value::Undefined
        } else {
            named_group_object(names, indices_pair)
        };
        indices.properties.borrow_mut().set("groups", named);
        result
            .properties
            .borrow_mut()
            .set("indices", Value::Array(indices));
    }
    Value::Array(result)
}

/// The text of a UTF-16 range, as a JavaScript string.
pub(super) fn utf16_slice(subject: &[u16], range: std::ops::Range<usize>) -> String {
    String::from_utf16_lossy(&subject[range])
}

/// Iterates code points while retaining an unpaired surrogate as its original one-unit string.
pub(super) fn string_code_point_units(text: &StringUnits) -> Vec<StringUnits> {
    let units: Vec<u16> = text.units().collect();
    let mut out = Vec::with_capacity(units.len());
    let mut at = 0;
    while at < units.len() {
        let end = if (0xd800..=0xdbff).contains(&units[at])
            && units
                .get(at + 1)
                .is_some_and(|unit| (0xdc00..=0xdfff).contains(unit))
        {
            at + 2
        } else {
            at + 1
        };
        out.push(StringUnits::from_units(&units[at..end]));
        at = end;
    }
    out
}

pub(super) fn value_to_string_units(value: &Value) -> Result<Vec<u16>, ExecutionError> {
    match value {
        Value::String(text) => Ok(text.units().collect()),
        other => Ok(other.to_string_units_checked()?.units().collect()),
    }
}

pub(super) fn string_case_map(text: &StringUnits, upper: bool) -> StringUnits {
    let units: Vec<u16> = text.units().collect();
    if let Ok(scalar) = String::from_utf16(&units) {
        return StringUnits::from(if upper {
            scalar.to_uppercase()
        } else {
            scalar.to_lowercase()
        });
    }
    let mut out = Vec::with_capacity(units.len());
    let mut segment = String::new();
    for decoded in char::decode_utf16(units.iter().copied()) {
        match decoded {
            Ok(character) => segment.push(character),
            Err(error) => {
                let mapped = if upper {
                    segment.to_uppercase()
                } else {
                    segment.to_lowercase()
                };
                out.extend(mapped.encode_utf16());
                segment.clear();
                out.push(error.unpaired_surrogate());
            }
        }
    }
    let mapped = if upper {
        segment.to_uppercase()
    } else {
        segment.to_lowercase()
    };
    out.extend(mapped.encode_utf16());
    StringUnits::from_units(&out)
}

pub(super) fn string_trim_units(
    text: &StringUnits,
    trim_start: bool,
    trim_end: bool,
) -> StringUnits {
    let units: Vec<u16> = text.units().collect();
    let mut start = 0;
    let mut end = units.len();
    if trim_start {
        let mut offset = 0;
        for decoded in char::decode_utf16(units.iter().copied()) {
            match decoded {
                Ok(character) if is_js_whitespace(character) => offset += character.len_utf16(),
                _ => break,
            }
        }
        start = offset;
    }
    if trim_end {
        let mut offset = 0;
        for decoded in char::decode_utf16(units.iter().copied()) {
            let length = match &decoded {
                Ok(character) => character.len_utf16(),
                Err(_) => 1,
            };
            match decoded {
                Ok(character) if is_js_whitespace(character) => {}
                Ok(character) => end = offset + character.len_utf16(),
                Err(_) => end = offset + 1,
            }
            offset += length;
        }
    }
    StringUnits::from_units(&units[start.min(end)..end])
}

/// The methods on `RegExp.prototype`, without the per-instance data a regexp also answers to.
///
/// A program reaches for these to call one against a receiver of its own -- `RegExp.prototype
/// .test.call(re, s)` is how a library avoids a monkey-patched `test`.
pub(super) fn regexp_method(name: &str) -> Value {
    match name {
        "test" => Value::Native(Native::RegExpTest, RealmId::PRIMARY),
        "exec" => Value::Native(Native::RegExpExec, RealmId::PRIMARY),
        "toString" => Value::Native(Native::RegExpToString, RealmId::PRIMARY),
        // What the accessors answer *for the table itself*. `RegExp.prototype` has no pattern
        // and no flags, and the specification gives these two an answer anyway rather than
        // throwing: the empty flag set, and the pattern that matches the empty string.
        "source" => Value::String("(?:)".to_owned().into()),
        "flags" => Value::String(String::new().into()),
        key => match RegExpSymbol::named(key) {
            Some(method) => Value::Native(Native::RegExpSymbol(method), RealmId::PRIMARY),
            None => Value::Undefined,
        },
    }
}

/// What `Date.prototype` publishes before the realm bootstrap runs: the two representation
/// operations the shared `Date.prototype` (`kiln-builtins/src/date.ts`) captures and builds
/// every other member on. It then redefines both, so a program never reads these markers
/// off the prototype.
pub(super) fn date_method(name: &str) -> Value {
    let method = match name {
        "getTime" => DateMethod::GetTime,
        "setTime" => DateMethod::SetTime,
        _ => return Value::Undefined,
    };
    Value::Native(Native::Date(method), RealmId::PRIMARY)
}

/// The methods on `ArrayBuffer.prototype` and `SharedArrayBuffer.prototype` this engine still
/// implements.
///
/// `byteLength`, `resizable` and `maxByteLength` are deliberately not here: each is an accessor
/// that reads a *buffer*, and the method table is not one.
pub(super) fn array_buffer_method(shared: bool, name: &str) -> Value {
    match name {
        // `resize`, `transfer` and `transferToFixedLength`, and the shared type's `grow` and
        // `slice`, are the shared bootstrap's (`kiln-builtins/src/arraybuffer.ts`), over
        // `__kilnArrayBufferResize` and `__kilnArrayBufferDetach`.
        "slice" if !shared => Value::Native(Native::ArrayBufferSlice, RealmId::PRIMARY),
        // Giving the bytes away and freezing them in one step. Only the unshared type again,
        // and for the same reason: a shared buffer's other holders cannot survive the giving.
        "transferToImmutable" if !shared => {
            Value::Native(Native::ArrayBufferTransferToImmutable, RealmId::PRIMARY)
        }
        // The copying half of the pair: the source keeps its bytes and the result cannot change.
        "sliceToImmutable" if !shared => {
            Value::Native(Native::ArrayBufferSliceToImmutable, RealmId::PRIMARY)
        }
        key if key == WellKnown::ToStringTag.property_key() => Value::String(
            if shared {
                "SharedArrayBuffer"
            } else {
                "ArrayBuffer"
            }
            .to_owned()
            .into(),
        ),
        _ => Value::Undefined,
    }
}

/// What every iterator this realm makes inherits from `%IteratorPrototype%`.
///
/// The seven lazy helpers and the eight methods that consume an iterator, plus `@@dispose`.
/// `Iterator.concat`, `Iterator.zip` and `Iterator.zipKeyed` are absent, and are not on this
/// table anyway: they are statics on the constructor rather than methods on the prototype.
pub(super) fn iterator_method(name: &str) -> Value {
    match name {
        // An iterator is its own iterable, and that is `%IteratorPrototype%`'s to say -- every
        // built-in iterator used to carry an own copy, which is one more own property than any
        // of them has.
        key if key == WellKnown::Iterator.property_key() => {
            Value::Native(Native::IteratorSelf, RealmId::PRIMARY)
        }
        "map" => Value::Native(Native::IteratorLazy(HelperKind::Map), RealmId::PRIMARY),
        "filter" => Value::Native(Native::IteratorLazy(HelperKind::Filter), RealmId::PRIMARY),
        "take" => Value::Native(Native::IteratorLazy(HelperKind::Take), RealmId::PRIMARY),
        "drop" => Value::Native(Native::IteratorLazy(HelperKind::Drop), RealmId::PRIMARY),
        "flatMap" => Value::Native(Native::IteratorLazy(HelperKind::FlatMap), RealmId::PRIMARY),
        "reduce" => Value::Native(
            Native::IteratorTerminal(TerminalKind::Reduce),
            RealmId::PRIMARY,
        ),
        "toArray" => Value::Native(
            Native::IteratorTerminal(TerminalKind::ToArray),
            RealmId::PRIMARY,
        ),
        "forEach" => Value::Native(
            Native::IteratorTerminal(TerminalKind::ForEach),
            RealmId::PRIMARY,
        ),
        "some" => Value::Native(
            Native::IteratorTerminal(TerminalKind::Some),
            RealmId::PRIMARY,
        ),
        "every" => Value::Native(
            Native::IteratorTerminal(TerminalKind::Every),
            RealmId::PRIMARY,
        ),
        "find" => Value::Native(
            Native::IteratorTerminal(TerminalKind::Find),
            RealmId::PRIMARY,
        ),
        "includes" => Value::Native(
            Native::IteratorTerminal(TerminalKind::Includes),
            RealmId::PRIMARY,
        ),
        "constructor" => Value::Native(Native::IteratorConstructor, RealmId::PRIMARY),
        // An iterator is its own iterable, which is what lets one be handed straight to a
        // `for-of` or to `Array.from`.
        key if key == WellKnown::Iterator.property_key() => {
            Value::Native(Native::IteratorSelf, RealmId::PRIMARY)
        }
        _ => Value::Undefined,
    }
}

/// What a `DataView` inherits from `DataView.prototype`.
///
/// The `Float16` pair is absent rather than approximated: half precision is not one of the
/// element types this engine's buffers know how to read, and a `getFloat16` that answered a
/// `Float32` decode of the same bytes would be a wrong answer no program could see coming.
pub(super) fn data_view_method(name: &str) -> Value {
    match name {
        "getInt8" => Value::Native(Native::DataViewGet(TypedKind::Int8), RealmId::PRIMARY),
        "getUint8" => Value::Native(Native::DataViewGet(TypedKind::Uint8), RealmId::PRIMARY),
        "getInt16" => Value::Native(Native::DataViewGet(TypedKind::Int16), RealmId::PRIMARY),
        "getUint16" => Value::Native(Native::DataViewGet(TypedKind::Uint16), RealmId::PRIMARY),
        "getInt32" => Value::Native(Native::DataViewGet(TypedKind::Int32), RealmId::PRIMARY),
        "getUint32" => Value::Native(Native::DataViewGet(TypedKind::Uint32), RealmId::PRIMARY),
        "getFloat16" => Value::Native(Native::DataViewGet(TypedKind::Float16), RealmId::PRIMARY),
        "getFloat32" => Value::Native(Native::DataViewGet(TypedKind::Float32), RealmId::PRIMARY),
        "getFloat64" => Value::Native(Native::DataViewGet(TypedKind::Float64), RealmId::PRIMARY),
        "setInt8" => Value::Native(Native::DataViewSet(TypedKind::Int8), RealmId::PRIMARY),
        "setUint8" => Value::Native(Native::DataViewSet(TypedKind::Uint8), RealmId::PRIMARY),
        "setInt16" => Value::Native(Native::DataViewSet(TypedKind::Int16), RealmId::PRIMARY),
        "setUint16" => Value::Native(Native::DataViewSet(TypedKind::Uint16), RealmId::PRIMARY),
        "setInt32" => Value::Native(Native::DataViewSet(TypedKind::Int32), RealmId::PRIMARY),
        "setUint32" => Value::Native(Native::DataViewSet(TypedKind::Uint32), RealmId::PRIMARY),
        "setFloat16" => Value::Native(Native::DataViewSet(TypedKind::Float16), RealmId::PRIMARY),
        "setFloat32" => Value::Native(Native::DataViewSet(TypedKind::Float32), RealmId::PRIMARY),
        "setFloat64" => Value::Native(Native::DataViewSet(TypedKind::Float64), RealmId::PRIMARY),
        "getBigInt64" => Value::Native(Native::DataViewGetBig { signed: true }, RealmId::PRIMARY),
        "getBigUint64" => Value::Native(Native::DataViewGetBig { signed: false }, RealmId::PRIMARY),
        "setBigInt64" => Value::Native(Native::DataViewSetBig { signed: true }, RealmId::PRIMARY),
        "setBigUint64" => Value::Native(Native::DataViewSetBig { signed: false }, RealmId::PRIMARY),
        key if key == WellKnown::ToStringTag.property_key() => {
            Value::String("DataView".to_owned().into())
        }
        _ => Value::Undefined,
    }
}

/// What every object inherits from `Object.prototype`.
///
/// `toString` and `valueOf` are deliberately absent from the *fallback* an ordinary object gets:
/// `ToPrimitive` looks for a `toString` and would start finding this one, changing what `"" + {}`
/// and every comparison against an object produce. They are reachable through
/// `Object.prototype.toString`, which is where programs actually use them.
pub(super) fn object_method_value(
    method: kiln_builtin_surface::ObjectMethod,
    realm: RealmId,
) -> Value {
    match method {
        kiln_builtin_surface::ObjectMethod::HasOwnProperty => {
            Value::Native(Native::ObjectHasOwnProperty, realm)
        }
        kiln_builtin_surface::ObjectMethod::IsPrototypeOf => {
            Value::Native(Native::ObjectIsPrototypeOf, realm)
        }
        kiln_builtin_surface::ObjectMethod::PropertyIsEnumerable => {
            Value::Native(Native::ObjectPropertyIsEnumerable, realm)
        }
        kiln_builtin_surface::ObjectMethod::ToString => {
            Value::Native(Native::ObjectToStringTag, realm)
        }
        kiln_builtin_surface::ObjectMethod::ToLocaleString => {
            Value::Native(Native::ObjectToLocaleString, realm)
        }
        kiln_builtin_surface::ObjectMethod::ValueOf => Value::Native(Native::ObjectValueOf, realm),
    }
}

pub(super) fn object_property(name: &str) -> Value {
    let Some(member) = kiln_builtin_surface::installed_prototype_member(
        kiln_builtin_surface::PrototypeId::Object,
        name,
    ) else {
        return Value::Undefined;
    };
    match member.binding {
        kiln_builtin_surface::SemanticBinding::ObjectMethod(method) => {
            object_method_value(method, RealmId::PRIMARY)
        }
        _ => Value::Undefined,
    }
}

/// What an ordinary object inherits from `Object.prototype`.
///
/// `toString` and `valueOf` are here now that `ToPrimitive` runs on the machine and tries them in
/// the specification's order: `String({})` finds `toString` and answers "[object Object]", and
/// `{} - 1` finds `valueOf`, gets the object back, and moves on to `toString`. While the
/// reduction was a pure function these two could not be here, because it would have found them
/// and had no way to call them.
pub(super) fn inherited_object_property(
    intrinsics: &Intrinsics,
    name: &str,
    realm: RealmId,
) -> Value {
    table_step(
        intrinsics,
        PrototypeTable::Object,
        name,
        tabulated_object_property(name),
        realm,
    )
}

/// The same step with the realm's store left out: what `Object.prototype` was *built* with.
///
/// Kept apart from the read above because [`Native::property`] is the table itself -- the thing
/// the store is a difference against -- and a table that consulted the difference would answer
/// with it twice.
/// What a built-in publishes as its *own* member, or `None` if it has none by that name.
///
/// Deliberately not [`Native::property`], which is the *read* path: a prototype table's last step
/// is up to `Object.prototype`, so a read of `Array.prototype.valueOf` finds one, as it must.
/// Asking that question about ownership made every built-in prototype in the realm claim all
/// eleven of `Object.prototype`'s members as its own -- which `getOwnPropertyNames` denied in the
/// same breath, so the two answers depended on which one you asked.
///
/// A member is inherited exactly when the value found *is* the one `Object.prototype` supplies.
/// Comparing the values rather than asking whether the table's generic arm answered is what keeps
/// the tables whose members are published by earlier, more specific arms -- `EventEmitter` and the
/// two stream tables -- owning what they publish.
pub(super) fn intrinsic_owns(native: Native, key: &str) -> Option<Value> {
    if matches!(native, Native::TypedArrayConstructor(_)) && matches!(key, "from" | "of") {
        // These are members of `%TypedArray%` inherited through the constructor's [[Prototype]],
        // even though `Native::property` answers them for direct reads on concrete constructors.
        return None;
    }
    if matches!(native, Native::PrototypeOf(PrototypeTable::Array)) {
        if key == WellKnown::Unscopables.property_key() {
            return Some(Value::Null);
        }
        if key == "length" {
            return Some(Value::Number(0.0));
        }
    }
    let found = native.property(key)?;
    if matches!(found, Value::Undefined) {
        return None;
    }
    let Native::PrototypeOf(table) = native else {
        return Some(found);
    };
    // `Object.prototype` owns its own members; it is the table every other one inherits from.
    if matches!(table, PrototypeTable::Object) {
        return Some(found);
    }
    let inherited = tabulated_object_property(key);
    if !matches!(inherited, Value::Undefined) && strict_equal(&found, &inherited) {
        return None;
    }
    // The five iterator tables and the two generator ones take their step up to
    // `%IteratorPrototype%` inside their own answer, so what it supplies reaches them the same
    // way `Object.prototype`'s does -- and is inherited for the same reason.
    if matches!(
        table,
        PrototypeTable::IteratorFor(_) | PrototypeTable::Generator | PrototypeTable::AsyncGenerator
    ) {
        let from_iterator = iterator_method(key);
        if !matches!(from_iterator, Value::Undefined) && strict_equal(&found, &from_iterator) {
            return None;
        }
    }
    Some(found)
}

pub(super) fn tabulated_object_property(name: &str) -> Value {
    let Some(member) = kiln_builtin_surface::installed_prototype_member(
        kiln_builtin_surface::PrototypeId::Object,
        name,
    ) else {
        return Value::Undefined;
    };
    match member.binding {
        kiln_builtin_surface::SemanticBinding::Constructor => {
            Value::Native(Native::ObjectConstructor, RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::ObjectMethod(method) => {
            object_method_value(method, RealmId::PRIMARY)
        }
        _ => Value::Undefined,
    }
}

/// Carry a method-table read its last step, up to `Object.prototype`.
///
/// Every built-in table but `Object.prototype`'s own sits *above* it in the chain, so a name the
/// table does not answer is still an ordinary object method. Without this step
/// `Array.prototype.hasOwnProperty` -- and, through it, every array and every collection --
/// reads as undefined, which is the shape the plain-object check every library ships depends on.
pub(super) fn inherit_from_object(table: PrototypeTable, found: Value, name: &str) -> Value {
    match found {
        Value::Undefined if !matches!(table, PrototypeTable::Object) => {
            tabulated_object_property(name)
        }
        found => found,
    }
}

/// What `Boolean.prototype` answers, and what a boolean primitive finds through it.
pub(super) fn boolean_method(name: &str) -> Value {
    let Some(member) = kiln_builtin_surface::installed_prototype_member(
        kiln_builtin_surface::PrototypeId::Boolean,
        name,
    ) else {
        return Value::Undefined;
    };
    match member.binding {
        kiln_builtin_surface::SemanticBinding::Constructor => {
            Value::Native(Native::BooleanConstructor, RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::BooleanMethod(
            kiln_builtin_surface::BooleanMethod::ToString,
        ) => Value::Native(Native::BooleanToString, RealmId::PRIMARY),
        kiln_builtin_surface::SemanticBinding::BooleanMethod(
            kiln_builtin_surface::BooleanMethod::ValueOf,
        ) => Value::Native(Native::BooleanValueOf, RealmId::PRIMARY),
        _ => Value::Undefined,
    }
}

/// What `BigInt.prototype` answers.
pub(super) fn bigint_method(name: &str) -> Value {
    let Some(member) = kiln_builtin_surface::installed_prototype_member(
        kiln_builtin_surface::PrototypeId::BigInt,
        name,
    ) else {
        return Value::Undefined;
    };
    match member.binding {
        kiln_builtin_surface::SemanticBinding::Constructor => {
            Value::Native(Native::BigIntConstructor, RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::BigIntMethod(
            kiln_builtin_surface::BigIntMethod::ToString
            | kiln_builtin_surface::BigIntMethod::ToLocaleString,
        ) => Value::Native(Native::BigIntToString, RealmId::PRIMARY),
        kiln_builtin_surface::SemanticBinding::BigIntMethod(
            kiln_builtin_surface::BigIntMethod::ValueOf,
        ) => Value::Native(Native::BigIntValueOf, RealmId::PRIMARY),
        kiln_builtin_surface::SemanticBinding::StringTag(tag) => {
            Value::String(tag.to_owned().into())
        }
        _ => Value::Undefined,
    }
}

/// What `Promise.prototype` answers, and what a promise finds through it.
pub(super) fn promise_method(name: &str) -> Value {
    let Some(member) = kiln_builtin_surface::installed_prototype_member(
        kiln_builtin_surface::PrototypeId::Promise,
        name,
    ) else {
        return Value::Undefined;
    };
    match member.binding {
        kiln_builtin_surface::SemanticBinding::Constructor => {
            Value::Native(Native::PromiseConstructor, RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::PromiseMethod(
            kiln_builtin_surface::PromiseMethod::Then,
        ) => Value::Native(Native::PromiseThen, RealmId::PRIMARY),
        kiln_builtin_surface::SemanticBinding::PromiseMethod(
            kiln_builtin_surface::PromiseMethod::Catch,
        ) => Value::Native(Native::PromiseCatch, RealmId::PRIMARY),
        kiln_builtin_surface::SemanticBinding::PromiseMethod(
            kiln_builtin_surface::PromiseMethod::Finally,
        ) => Value::Native(Native::PromiseFinally, RealmId::PRIMARY),
        kiln_builtin_surface::SemanticBinding::StringTag(tag) => {
            Value::String(tag.to_owned().into())
        }
        _ => Value::Undefined,
    }
}

/// The object `new f()` gives instances, created on first use.
///
/// Created lazily so an ordinary function never used as a constructor never allocates one, and
/// carrying `constructor` back to the function -- which is what makes `x.constructor === C` true
/// and what a console reads to print `C {}` rather than `{}`. The two point at each other, so a
/// class that is never collected keeps its prototype alive; a realm's constructors outlive the
/// program anyway.
/// Which generator table a function's `prototype` object inherits from, or `None` for an
/// ordinary function whose `prototype` is an ordinary object.
/// What `%GeneratorPrototype%` and `%AsyncGeneratorPrototype%` answer.
///
/// The three methods are the same `Callback`s a generator value already resolves by name, so a
/// program that takes one off the prototype and calls it on a generator gets the same function
/// the generator hands back itself.
pub(super) fn generator_prototype_method(asynchronous: bool, name: &str) -> Value {
    match name {
        "next" if asynchronous => {
            Value::Callback(CallbackMethod::AsyncGeneratorNext, RealmId::PRIMARY)
        }
        "return" if asynchronous => {
            Value::Callback(CallbackMethod::AsyncGeneratorReturn, RealmId::PRIMARY)
        }
        "throw" if asynchronous => {
            Value::Callback(CallbackMethod::AsyncGeneratorThrow, RealmId::PRIMARY)
        }
        "next" => Value::Callback(CallbackMethod::GeneratorNext, RealmId::PRIMARY),
        "return" => Value::Callback(CallbackMethod::GeneratorReturn, RealmId::PRIMARY),
        "throw" => Value::Callback(CallbackMethod::GeneratorThrow, RealmId::PRIMARY),
        key if key == WellKnown::ToStringTag.property_key() => Value::String(
            if asynchronous {
                "AsyncGenerator"
            } else {
                "Generator"
            }
            .to_owned()
            .into(),
        ),
        _ => Value::Undefined,
    }
}

pub(super) fn generator_prototype_table(closure: &Rc<Closure>) -> Option<PrototypeTable> {
    let proto = closure.program.functions.get(closure.function as usize)?;
    if proto.async_generator {
        Some(PrototypeTable::AsyncGenerator)
    } else if proto.generator {
        Some(PrototypeTable::Generator)
    } else {
        None
    }
}

/// The distinct built-in constructor inherited by each function kind. Async function bodies are
/// lowered through the generator execution machinery, but that implementation detail must not
/// make their observable constructor or `instanceof` chain a GeneratorFunction.
pub(super) fn function_kind_prototype(closure: &Rc<Closure>) -> Option<Native> {
    let proto = closure.program.functions.get(closure.function as usize)?;
    if proto.async_generator {
        Some(Native::AsyncGeneratorFunctionPrototype)
    } else if proto.asynchronous {
        Some(Native::AsyncFunctionPrototype)
    } else if proto.generator {
        Some(Native::GeneratorFunctionPrototype)
    } else {
        None
    }
}

pub(super) fn prototype_object(closure: &Rc<Closure>) -> Value {
    let mut slot = closure.prototype.borrow_mut();
    if let Some(existing) = slot.as_ref() {
        return existing.clone();
    }
    let mut data = ObjectData::default();
    // A generator function's `prototype` is not an ordinary one: it carries no `constructor` at
    // all, and it is the object its generators inherit from -- so it points at
    // `%GeneratorPrototype%` rather than at `Object.prototype`. That is the link the chain was
    // missing.
    match generator_prototype_table(closure) {
        Some(table) => {
            data.set_prototype(Value::Native(Native::PrototypeOf(table), RealmId::PRIMARY));
        }
        // Not enumerated: the link back is the realm's, so `Object.keys(C.prototype)` is empty
        // for a class that declared nothing and `for (const k in new C())` walks the fields
        // alone.
        None => data.set_non_enumerable("constructor", Value::Function(Rc::clone(closure))),
    }
    let prototype = Value::Object(Rc::new(RefCell::new(data)));
    *slot = Some(prototype.clone());
    prototype
}

/// The elements a typed array's constructor or `set` was given.
///
/// Left as values rather than flattened to numbers: whether an element is a Number or a BigInt
/// is what decides whether writing it through the destination is a conversion or a `TypeError`,
/// so collapsing the two here would decide that question in the wrong place.
pub(super) fn typed_source(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => dense_items(&items.borrow()),
        Some(Value::TypedArray(view)) => view.items(),
        _ => Vec::new(),
    }
}

/// Refuse a copy between views whose elements are in different numeric towers.
///
/// Checked on the pair rather than element by element, because that is where the specification
/// checks it: `ToBigInt` is never reached for a typed-array source, so the refusal has to come
/// from the kinds themselves and has to hold for an empty source too.
pub(super) fn same_content_type(
    kind: TypedKind,
    source: Option<&Value>,
) -> Result<(), ExecutionError> {
    if let Some(Value::TypedArray(view)) = source
        && view.kind.is_big() != kind.is_big()
    {
        return Err(ExecutionError::TypedArrayContentTypeMismatch);
    }
    Ok(())
}

/// `start` and `end` from a pair of possibly-negative, possibly-absent bounds.
pub(super) fn slice_bounds(arguments: &[Value], length: usize) -> (usize, usize) {
    let resolve = |value: Option<&Value>, fallback: usize| -> usize {
        match value {
            None | Some(Value::Undefined) => fallback,
            Some(value) => {
                let at = value.as_number();
                if at.is_nan() {
                    return 0;
                }
                if !at.is_finite() {
                    return if at < 0.0 { 0 } else { length };
                }
                if at < 0.0 {
                    (length as f64 + at).max(0.0) as usize
                } else {
                    (at as usize).min(length)
                }
            }
        }
    };
    let start = resolve(arguments.first(), 0);
    let end = resolve(arguments.get(1), length);
    (start, end.max(start))
}

/// `new Uint8Array(...)` in each of its four shapes.
///
/// A length allocates; an array or another view copies; an `ArrayBuffer` *shares*, which is the
/// shape that makes two views over one buffer see each other's writes.
pub(super) fn construct_typed_array(
    kind: TypedKind,
    arguments: &[Value],
) -> Result<Value, ExecutionError> {
    let width = kind.width();
    Ok(match arguments.first() {
        Some(Value::ArrayBuffer(buffer)) => {
            // A view onto a buffer with no bytes is not an empty view but an impossible one:
            // the specification refuses it outright rather than handing back something that
            // reports length zero and could never report anything else.
            if buffer.detached() {
                return Err(ExecutionError::BufferDetached);
            }
            let total = buffer.byte_length();
            let offset = bounded_buffer(to_index(arguments.get(1))?, 1)?;
            if offset > total || offset % width != 0 {
                return Err(ExecutionError::InvalidArrayLength);
            }
            let length = match arguments.get(2) {
                // No explicit length over a *resizable* buffer is the specification's
                // length-tracking view: it has no count of its own, so a later resize moves its
                // end rather than leaving it addressing bytes that no longer exist. Over a
                // fixed buffer the two readings coincide, and the count is recorded because
                // nothing can ever change it.
                None | Some(Value::Undefined) => {
                    let remaining = total - offset;
                    // A length-tracking view has no count to be wrong: its end follows the
                    // buffer, and `length` is the floor of what fits. The exact-multiple rule
                    // belongs to a *fixed* buffer, where a leftover byte would mean a view
                    // addressing a partial element for ever. Applying it to both made
                    // `new Int32Array(new ArrayBuffer(10, { maxByteLength: 20 }))` a RangeError
                    // where every other engine answers a view of length two -- and a resizable
                    // buffer is normally sized in bytes, so a non-multiple is the ordinary case
                    // rather than the odd one.
                    if buffer.max_byte_length.is_some() {
                        None
                    } else {
                        if remaining % width != 0 {
                            return Err(ExecutionError::InvalidArrayLength);
                        }
                        Some(remaining / width)
                    }
                }
                Some(value) => {
                    let length = bounded_buffer(to_index(Some(value))?, width)?;
                    if length > (total - offset) / width {
                        return Err(ExecutionError::InvalidArrayLength);
                    }
                    Some(length)
                }
            };
            Value::TypedArray(Rc::new(TypedArrayData {
                kind,
                buffer: Rc::clone(buffer),
                offset,
                length,
                properties: RefCell::new(ObjectData::default()),
            }))
        }
        Some(Value::Array(_) | Value::TypedArray(_)) => {
            same_content_type(kind, arguments.first())?;
            let items = typed_source(arguments.first());
            let view = TypedArrayData {
                kind,
                buffer: BufferData::fixed(bounded_buffer(items.len() as f64, width)? * width),
                offset: 0,
                length: Some(items.len()),
                properties: RefCell::new(ObjectData::default()),
            };
            for (index, item) in items.into_iter().enumerate() {
                view.set(index, &item)?;
            }
            Value::TypedArray(Rc::new(view))
        }
        other => {
            let length = bounded_buffer(to_index(other)?, width)?;
            Value::TypedArray(Rc::new(TypedArrayData {
                kind,
                buffer: BufferData::fixed(length * width),
                offset: 0,
                length: Some(length),
                properties: RefCell::new(ObjectData::default()),
            }))
        }
    })
}
/// `StringToBigInt`, which is `StringNumericLiteral` without the decimal forms.
///
/// The prefixed radices count -- `0x`, `0o`, `0b`, in either case -- and were the whole of what
/// was missing: `BigInt("0x10")` is 16, and parsing the trimmed text as decimal refused it. A
/// sign is allowed only on the decimal form, which is why `-0x10` is still a SyntaxError, and
/// the empty string is zero rather than an error, exactly as `Number("")` is.
pub(super) fn string_to_big_int(text: &str) -> Option<num_bigint::BigInt> {
    let trimmed = text.trim_matches(is_js_whitespace);
    if trimmed.is_empty() {
        return Some(num_bigint::BigInt::from(0));
    }
    let radix = match trimmed.get(..2) {
        Some("0x" | "0X") => Some(16),
        Some("0o" | "0O") => Some(8),
        Some("0b" | "0B") => Some(2),
        _ => None,
    };
    match radix {
        // A prefixed literal takes no sign and no separators, so the digits are handed over
        // exactly as written and an empty tail is refused rather than read as zero.
        Some(radix) => {
            let digits = trimmed.get(2..).filter(|digits| !digits.is_empty())?;
            num_bigint::BigInt::parse_bytes(digits.as_bytes(), radix)
        }
        None => trimmed.parse::<num_bigint::BigInt>().ok(),
    }
}
/// A regexp's flags in the order `RegExp.prototype.flags` publishes them.
///
/// `dgimsuvy`, which is the order the generic getter walks its eight accessors in. The stored
/// spelling is whatever the program wrote, and the two must not disagree.
pub(super) fn canonical_regexp_flags(written: &str) -> String {
    "dgimsuvy"
        .chars()
        .filter(|flag| written.contains(*flag))
        .collect()
}

/// Whether `target` carries the brand of the class evaluation that is asking.
///
/// A private name is stored under a key fixed at compile time, so two *evaluations* of one class
/// share it and the own-only rule alone cannot tell them apart: `A.probe(new B())` read B's slot
/// where the language refuses it. Each evaluation mints a brand, every instance and the
/// constructor carry it, and this compares the two.
///
/// An absent expected brand means the access was compiled outside any class -- a syntax error
/// reported elsewhere -- and is let through so the error that matters is the one already raised.
pub(super) fn private_brand_matches(target: &Value, name: &str, expected: &Value) -> bool {
    if matches!(expected, Value::Undefined) {
        return true;
    }
    let Some((scope, _)) = name
        .strip_prefix(PRIVATE_KEY_PREFIX)
        .and_then(|rest| rest.split_once(':'))
    else {
        return true;
    };
    let key = format!("{PRIVATE_KEY_PREFIX}{scope}:%brand");
    match own_private_element(target, &key) {
        Some((Property::Value(held), _)) => strict_equal(&held, expected),
        _ => false,
    }
}

/// Whether `new value(...)` is something the language will run.
///
/// A different question from "is it callable", and programs ask it directly: Test262's own
/// `isConstructor` is `Reflect.construct(function () {}, [], f)`, so hundreds of tests turn on
/// this answer. An arrow, a generator and an async function are callable and have no
/// `[[Construct]]`; so is every built-in that is not a constructor.
///
/// A method shorthand is the one shape this cannot tell apart from an ordinary function: the
/// compiler does not record which it is, so `({ m() {} }).m` is reported constructable here and
/// is not. That is a gap in what the IR carries, not a judgement made here.
/// The characters `trim`, `trimStart` and `trimEnd` remove.
///
/// Rust's `char::is_whitespace` is Unicode `White_Space`, and the language's `WhiteSpace`
/// production is that set *plus* U+FEFF, which Unicode classifies as a format character rather
/// than whitespace. So `"\u{feff}a\u{feff}".trim()` kept both ends here and strips them
/// everywhere else -- a byte-order mark at the front of a file read as text is exactly how one
/// arrives, and it survived into a value the program then compared against.
pub(super) fn is_js_whitespace(character: char) -> bool {
    character.is_whitespace() || character == '\u{feff}'
}

pub(super) fn closure_is_constructor(closure: &Closure) -> bool {
    closure
        .program
        .functions
        .get(closure.function as usize)
        .is_some_and(|proto| {
            !proto.arrow && !proto.generator && !proto.asynchronous && !proto.is_method
        })
}

pub(super) fn is_constructor(value: &Value) -> bool {
    match value {
        Value::Function(closure) => closure_is_constructor(closure),
        // A bound function is constructable exactly when what it wraps is: `new f.bind(x)()`
        // runs `f` as a constructor and ignores the bound receiver.
        Value::Bound(bound) => is_constructor(&bound.target),
        Value::Proxy(proxy) => is_constructor(&proxy.target),
        Value::Native(native, _) => native_is_constructor(*native),
        _ => false,
    }
}

/// The same question for a built-in.
pub(super) fn native_is_constructor(native: Native) -> bool {
    match native {
        // Byte text conversions are built-in methods, not function constructors. They have no
        // [[Construct]] slot even though their observable properties are projected from a native
        // marker in the interpreter.
        Native::Uint8Text(_) => false,
        // `Proxy` is the one constructor with no `prototype` of its own -- a proxy inherits the
        // target's chain -- so the rule below cannot see it.
        Native::ProxyConstructor
        | Native::TextDecoderConstructor
        | Native::TextEncoderConstructor
        | Native::AbortControllerConstructor
        // `AbortSignal` is a constructor that *refuses* `new`, exactly as `Symbol` is: a signal
        // comes from a controller or from one of the statics and never from a caller, which is
        // what keeps cancellation one-directional. Reported as a constructor so the refusal is
        // "Illegal constructor" rather than "not a constructor", which is what node says and
        // what a feature check reads.
        | Native::AbortSignalConstructor
        | Native::NodeStringDecoderConstructor
        | Native::NodeAsyncResource
        // Three more the rule below cannot see, because they have no intrinsic prototype entry
        // for it to find. Each is a real constructor, and saying otherwise is visible to a
        // program: `Reflect.construct(URL, ["http://example.com"])` answered "URL is not a
        // constructor", and `new http.Server()` the same. `Iterator` is the abstract base and
        // *refuses* `new`, like `AbortSignal` above -- reported as a constructor so the refusal
        // names the right reason.
        | Native::UrlConstructor
        | Native::IteratorConstructor
        | Native::ConsoleConstructor
        | Native::NodeHttpServer
        | Native::NodeAsyncLocalStorage
        | Native::NodeBufferGlobal
        | Native::WebStub(_) => true,
        Native::AsyncFunctionPrototype
        | Native::GeneratorFunctionPrototype
        | Native::AsyncGeneratorFunctionPrototype => false,
        // Each error kind's `prototype` is a per-realm object rather than a table, which the
        // marker alone cannot answer for.
        Native::Error(_) => true,
        // Everything else that constructs is exactly what owns a `prototype`: that object is
        // what its instances link to, and a built-in with nothing to link is not one. It is
        // also what separates `Symbol` -- a constructor that refuses `new` -- from `Math.max`,
        // which is not a constructor at all.
        other => intrinsic_prototype(&Value::Native(other, RealmId::PRIMARY)).is_some(),
    }
}

/// One accessor published by a built-in prototype, or by a constructor in the case of
/// `Symbol.species`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinAccessor {
    /// `ArrayBuffer.prototype`'s four, each a `TypeError` on a *shared* buffer: the two types
    /// share no accessor, and answering a shared buffer's length from the unshared getter would
    /// make the brand check the specification demands invisible.
    BufferByteLength,
    BufferMaxByteLength,
    BufferResizable,
    BufferDetached,
    BufferImmutable,
    /// `SharedArrayBuffer.prototype`'s three, the mirror image.
    SharedByteLength,
    SharedMaxByteLength,
    SharedGrowable,
    DataViewBuffer,
    DataViewByteLength,
    DataViewByteOffset,
    TypedArrayBuffer,
    TypedArrayByteLength,
    TypedArrayByteOffset,
    TypedArrayLength,
    /// `Map.prototype.size` and `Set.prototype.size`. The payload is which of the two, because
    /// each is branded: `Map.prototype`'s getter refuses a `Set`.
    CollectionSize(bool),
    /// `get [Symbol.species]`, which every one of them defines identically: it answers its own
    /// receiver, so a subclass's derived operations build the subclass.
    Species,
    /// `DisposableStack.prototype.disposed`.
    DisposableStackDisposed,
    /// `AsyncDisposableStack.prototype.disposed`.
    AsyncDisposableStackDisposed,
    /// `Symbol.prototype.description`, which is an accessor and not a slot on the instance.
    SymbolDescription,
}

impl BuiltinAccessor {
    /// The name the accessor is published under.
    pub(super) fn property_key(self) -> &'static str {
        match self {
            Self::BufferByteLength
            | Self::SharedByteLength
            | Self::DataViewByteLength
            | Self::TypedArrayByteLength => "byteLength",
            Self::BufferMaxByteLength | Self::SharedMaxByteLength => "maxByteLength",
            Self::BufferResizable => "resizable",
            Self::SharedGrowable => "growable",
            Self::BufferDetached => "detached",
            Self::BufferImmutable => "immutable",
            Self::DataViewBuffer | Self::TypedArrayBuffer => "buffer",
            Self::DataViewByteOffset | Self::TypedArrayByteOffset => "byteOffset",
            Self::TypedArrayLength => "length",
            Self::SymbolDescription => "description",
            Self::CollectionSize(_) => "size",
            Self::DisposableStackDisposed | Self::AsyncDisposableStackDisposed => "disposed",
            Self::Species => WellKnown::Species.property_key(),
        }
    }
}

/// Read a branded slot without consulting ordinary own properties or prototype tables.
///
/// Callers must perform the accessor's brand check first. DataView bounds errors are checked by
/// the dispatch path; this helper retains the zero used by the non-throwing internal property
/// reader.
pub(super) fn builtin_accessor_slot(accessor: BuiltinAccessor, receiver: &Value) -> Value {
    match accessor {
        BuiltinAccessor::BufferByteLength | BuiltinAccessor::SharedByteLength => match receiver {
            Value::ArrayBuffer(buffer) => Value::Number(buffer.byte_length() as f64),
            _ => unreachable!("buffer accessor passed a non-buffer receiver"),
        },
        BuiltinAccessor::BufferMaxByteLength | BuiltinAccessor::SharedMaxByteLength => {
            match receiver {
                Value::ArrayBuffer(buffer) => Value::Number(if buffer.detached() {
                    0.0
                } else {
                    buffer
                        .max_byte_length
                        .unwrap_or_else(|| buffer.byte_length()) as f64
                }),
                _ => unreachable!("buffer accessor passed a non-buffer receiver"),
            }
        }
        BuiltinAccessor::BufferResizable => match receiver {
            Value::ArrayBuffer(buffer) => Value::Boolean(buffer.max_byte_length.is_some()),
            _ => unreachable!("buffer accessor passed a non-buffer receiver"),
        },
        BuiltinAccessor::BufferDetached => match receiver {
            Value::ArrayBuffer(buffer) => Value::Boolean(buffer.detached()),
            _ => unreachable!("buffer accessor passed a non-buffer receiver"),
        },
        BuiltinAccessor::BufferImmutable => match receiver {
            Value::ArrayBuffer(buffer) => Value::Boolean(buffer.immutable),
            _ => unreachable!("buffer accessor passed a non-buffer receiver"),
        },
        BuiltinAccessor::SharedGrowable => match receiver {
            Value::ArrayBuffer(buffer) => Value::Boolean(buffer.max_byte_length.is_some()),
            _ => unreachable!("shared-buffer accessor passed a non-buffer receiver"),
        },
        BuiltinAccessor::DataViewBuffer => match receiver {
            Value::DataView(view) => Value::ArrayBuffer(Rc::clone(&view.buffer)),
            _ => unreachable!("DataView accessor passed a non-DataView receiver"),
        },
        BuiltinAccessor::DataViewByteLength => match receiver {
            Value::DataView(view) => Value::Number(view.byte_length().unwrap_or(0) as f64),
            _ => unreachable!("DataView accessor passed a non-DataView receiver"),
        },
        BuiltinAccessor::DataViewByteOffset => match receiver {
            Value::DataView(view) => Value::Number(if view.out_of_bounds() {
                0.0
            } else {
                view.offset as f64
            }),
            _ => unreachable!("DataView accessor passed a non-DataView receiver"),
        },
        BuiltinAccessor::TypedArrayBuffer => match receiver {
            Value::TypedArray(view) => Value::ArrayBuffer(Rc::clone(&view.buffer)),
            _ => unreachable!("TypedArray accessor passed a non-TypedArray receiver"),
        },
        BuiltinAccessor::TypedArrayByteLength => match receiver {
            Value::TypedArray(view) => Value::Number((view.len() * view.kind.width()) as f64),
            _ => unreachable!("TypedArray accessor passed a non-TypedArray receiver"),
        },
        BuiltinAccessor::TypedArrayByteOffset => match receiver {
            Value::TypedArray(view) => Value::Number(if view.out_of_bounds() {
                0.0
            } else {
                view.offset as f64
            }),
            _ => unreachable!("TypedArray accessor passed a non-TypedArray receiver"),
        },
        BuiltinAccessor::TypedArrayLength => match receiver {
            Value::TypedArray(view) => Value::Number(view.len() as f64),
            _ => unreachable!("TypedArray accessor passed a non-TypedArray receiver"),
        },
        BuiltinAccessor::SymbolDescription => match receiver {
            Value::Symbol(symbol) => symbol
                .description_units()
                .map_or(Value::Undefined, Value::String),

            _ => unreachable!("Symbol accessor passed a non-symbol receiver"),
        },
        BuiltinAccessor::Species
        | BuiltinAccessor::CollectionSize(_)
        | BuiltinAccessor::DisposableStackDisposed
        | BuiltinAccessor::AsyncDisposableStackDisposed => {
            unreachable!("stateful accessor handled before direct slot read")
        }
    }
}

/// Which accessor an intrinsic prototype publishes under `key`, or `None` if the name is not an
/// accessor.
///
/// One row in `intrinsic_members` carries both the name and the getter, because they used to be
/// two tables and a name present in one but absent from the other is not a compile error -- it is
/// a property that lists and describes as a data property holding `undefined`.

pub(super) fn prototype_accessor(table: PrototypeTable, key: &str) -> Option<BuiltinAccessor> {
    intrinsic_members::accessor(table, key)
}

/// The accessor the *machine* has to run for a read of `key` on `target`, or `None`.
///
/// These are the accessors whose getter does real work -- a brand check and a slot -- so a read
/// of one is a call. The tables cannot answer a call, which is why a read has to ask here
/// before it falls through to them.
pub(super) fn machine_accessor(target: &Value, key: &str) -> Option<BuiltinAccessor> {
    match target {
        Value::Native(Native::PrototypeOf(table), _) => prototype_accessor(*table, key),
        Value::Native(native, _) if defines_species(*native) => {
            (key == WellKnown::Species.property_key()).then_some(BuiltinAccessor::Species)
        }
        // Each concrete typed-array constructor inherits `%TypedArray%[@@species]`; unlike the
        // ordinary function case below, native constructors do not expose that inheritance via
        // a stored Function object, so answer the inherited accessor directly here.
        Value::Native(Native::TypedArrayConstructor(_), _)
            if key == WellKnown::Species.property_key() =>
        {
            Some(BuiltinAccessor::Species)
        }
        // A subclass constructor *inherits* the accessor: `class MyArray extends Array {}` puts
        // `Array` in `MyArray`'s own prototype chain, so `MyArray[Symbol.species]` is the same
        // getter answering a different receiver -- which is the whole mechanism by which
        // `myArray.map(f)` builds a `MyArray`. The chain here is the constructor's own, not its
        // instances', and it ends at a built-in that the store cannot be walked into.
        Value::Function(_) | Value::Bound(_) if key == WellKnown::Species.property_key() => {
            let mut current = prototype_of(target);
            for _ in 0..MAX_PROTOTYPE_DEPTH {
                match current {
                    Value::Native(native, _)
                        if defines_species(native)
                            || matches!(native, Native::TypedArrayConstructor(_)) =>
                    {
                        return Some(BuiltinAccessor::Species);
                    }
                    Value::Function(_) | Value::Bound(_) => current = prototype_of(&current),
                    _ => return None,
                }
            }
            None
        }
        _ => None,
    }
}

/// Whether a machine-backed accessor is an own property of `target`.
///
/// `machine_accessor` also follows the constructor inheritance that ordinary property reads
/// need. In particular, concrete typed-array constructors inherit `%TypedArray%[@@species]`.
/// Reflection over own keys and own descriptors must not turn that inherited getter into an own
/// property of every concrete constructor (or of functions inheriting one of those constructors).
pub(super) fn machine_own_accessor(target: &Value, key: &str) -> Option<BuiltinAccessor> {
    if key == WellKnown::Species.property_key()
        && matches!(
            target,
            Value::Native(Native::TypedArrayConstructor(_), _)
                | Value::Function(_)
                | Value::Bound(_)
        )
    {
        return None;
    }
    machine_accessor(target, key)
}

/// Whether this built-in defines `Symbol.species` through the interpreter's native accessor.
/// The typed-array abstract constructor installs its public getter in shared TypeScript instead;
/// the concrete view constructors inherit that stored getter through their actual prototype link.
pub(super) const fn defines_species(native: Native) -> bool {
    matches!(
        native,
        Native::ArrayConstructor
            | Native::ArrayBufferConstructor
            | Native::SharedArrayBufferConstructor
            | Native::MapConstructor
            | Native::SetConstructor
            | Native::PromiseConstructor
            | Native::RegExpConstructor
    )
}

/// Whether a numeric argument still has to go through `ToNumber` to become one.
///
/// `undefined` is left alone: it is not a missing number but a *meaningful* one, and each
/// built-in says for itself what it means -- `slice(1, undefined)` runs to the end, where
/// `ToNumber(undefined)` is `NaN`. A symbol and a BigInt are converted rather than skipped,
/// because converting either is the `TypeError` the language requires and the pure table
/// answered `NaN` for both.
pub(super) fn needs_number_conversion(value: &Value) -> bool {
    match value {
        Value::Undefined | Value::Number(_) => false,
        Value::Symbol(_) | Value::BigInt(_) => true,
        other => is_reflect_target(other),
    }
}

/// The argument positions a built-in reads as *text*, so `ToString` runs the program's own
/// `toString` before the built-in ever sees the value.
///
/// The same shape as [`numeric_arguments`] and for the same reason: the conversion can call back
/// into the program -- and can therefore throw, which is exactly what these are written to do --
/// and the pure table has no machine to run it with. It read every object as
/// `"[object Object]"` instead, so a `toString` that threw was silently skipped.
pub(super) const fn textual_arguments(native: Native) -> &'static [usize] {
    use Native as N;
    match native {
        N::Escape | N::Unescape => &[0],
        // The search string converts before the position does: `"s".indexOf(a, b)` is
        // `ToString(a)` then `ToIntegerOrInfinity(b)`, and running the numeric pass first
        // reversed a pair of conversions a program counting its own `toString` calls can see.
        N::StringIndexOf
        | N::StringLastIndexOf
        | N::StringIncludes
        | N::StringStartsWith
        | N::StringEndsWith
        // The form is `ToString`'d before it is checked, so an object naming a form is accepted
        // and one whose `toString` throws does so here.
        | N::StringNormalize => &[0],
        _ => &[],
    }
}

/// Whether an argument still has to go through `ToString`.
///
/// An object, because the conversion is a call it may define; and a symbol, because `ToString`
/// is the step that refuses one. Every other primitive spells itself, so it is left for the
/// built-in's own `to_display_string` and nothing is re-dispatched for it.
pub(super) fn needs_text_conversion(value: &Value) -> bool {
    matches!(value, Value::Symbol(_)) || is_reflect_target(value)
}

/// Whether this value reduces to a primitive by calling one of its own methods.
/// Whether reducing this value to a primitive is a *call* rather than a no-op.
///
/// The question `==` asks before it coerces at all, and deliberately not "is this an object": a
/// `Date` is a value kind here and still reduces by calling, so `date == String(date)` compares
/// two strings and is true. Answering it with an object test left that one false. The C backend
/// asks the same question under the same name, for the same reason.
pub(super) fn reduces_to_primitive(value: &Value) -> bool {
    matches!(
        value,
        Value::Object(_)
            | Value::Function(_)
            | Value::Array(_)
            | Value::Date(_)
            | Value::Temporal(_)
    )
}
