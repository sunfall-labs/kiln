use super::*;

/// `Reflect.get` does not box primitive targets. Every other public `Value` variant represents an
/// object or callable object; `Cell` is internal and must never escape as a target.
/// Whether `target` may be given `prototype`, per `OrdinarySetPrototypeOf`'s two refusals.
///
/// An object that has been made non-extensible has declared its shape final, and its prototype
/// is part of that shape. And a chain that reaches back to the object it started from is a
/// cycle: every failed lookup on it walks forever, which is a hang rather than an error, so the
/// language refuses to build one at all. Assigning the prototype an object already has is
/// always allowed -- nothing changes, so neither refusal has anything to refuse.
///
/// Answering `bool` rather than throwing is what lets the four spellings of this operation each
/// react as the specification says: `Reflect.setPrototypeOf` reports `false`, `Object.setPrototypeOf`
/// and the `__proto__` setter throw, and `{ __proto__: x }` in a literal never gets here at all.
pub(super) fn prototype_may_change(
    intrinsics: &Intrinsics,
    target: &Value,
    prototype: &Value,
) -> bool {
    if same_value(&prototype_of_in(intrinsics, target), prototype) {
        return true;
    }
    if !is_extensible_target(intrinsics, target) {
        return false;
    }
    let mut link = prototype.clone();
    // The chain being walked cannot already contain a cycle -- this is the check that keeps one
    // from being made -- so the walk terminates on its own. The step cap is there for the one
    // case that is not this operation's to reason about: a chain that reaches a value whose
    // prototype is answered by a table rather than a slot.
    for _ in 0..MAX_PROTOTYPE_DEPTH {
        if !is_reflect_target(&link) {
            return true;
        }
        if same_value(&link, target) {
            return false;
        }
        // A proxy answers `[[GetPrototypeOf]]` with a trap, and running the program's code in
        // the middle of this check is not what the specification asks for: it stops the walk at
        // the first non-ordinary object.
        if matches!(link, Value::Proxy(_)) {
            return true;
        }
        link = prototype_of_in(intrinsics, &link);
    }
    true
}

/// `prototype_of` plus the mutable link recorded for a native intrinsic in its realm store.
pub(super) fn prototype_of_in(intrinsics: &Intrinsics, target: &Value) -> Value {
    intrinsics
        .prototype_override(target)
        .unwrap_or_else(|| prototype_of(target))
}

pub(super) fn record_prototype_in(
    intrinsics: &Intrinsics,
    target: &Value,
    prototype: Value,
) -> bool {
    if intrinsic_key(target).is_some() {
        intrinsics.record_prototype(target, prototype)
    } else {
        record_prototype(target, prototype)
    }
}

/// How far the cycle check walks before it gives up and allows the write.
/// What `Object.setPrototypeOf` and the `__proto__` setter throw when the change is refused.
pub(super) fn prototype_change_refused() -> ExecutionError {
    ExecutionError::TypeErrorMessage {
        message: "Cyclic __proto__ value or non-extensible object".to_owned(),
    }
}

/// Which of a built-in's arguments name a *number*, in argument order.
///
/// Every one of these begins with `ToNumber`, and `ToNumber` of an object is the program's own
/// `Symbol.toPrimitive`, `valueOf` or `toString` -- a call, which the pure table cannot make.
/// It answered `NaN` for every object instead, so `a.slice({valueOf: () => 1})` silently sliced
/// from zero and `"ab".repeat({valueOf(){throw x}})` never threw at all.
///
/// In argument order because the order is observable: two arguments that both throw must throw
/// the leftmost first. Only positions that are numbers are listed -- `fill`'s value and
/// `indexOf`'s needle are any value at all, and converting either would be wrong.
pub(super) const fn numeric_arguments(native: Native) -> &'static [usize] {
    use Native as N;
    match native {
        N::ArraySlice
        | N::ArraySplice
        | N::StringSlice
        | N::StringSubstring
        | N::StringSubstr
        | N::ArrayBufferSlice
        | N::ArrayCopy(ArrayCopy::ToSpliced) => &[0, 1],
        N::ArrayCopyWithin => &[0, 1, 2],
        N::ArrayFill => &[1, 2],
        N::ArrayIndexOf
        | N::ArrayLastIndexOf
        | N::ArrayIncludes
        | N::StringIndexOf
        | N::StringLastIndexOf
        | N::StringIncludes
        | N::StringStartsWith
        | N::StringEndsWith => &[1],
        // `split` reads its *limit* before its separator: `ToUint32(limit)` happens first, and a
        // limit of zero answers the empty array without the string ever being cut. Leaving the
        // limit to the pure read meant the separator converted first, which a `toString`
        // counting its calls can see.
        N::StringSplit => &[1],
        N::ArrayAt
        | N::ArrayCopy(ArrayCopy::With)
        | N::StringAt
        | N::StringCharAt
        | N::StringCharCodeAt
        | N::StringCodePointAt
        | N::StringPadStart
        | N::StringPadEnd
        | N::StringRepeat
        | N::NumberToFixed
        | N::NumberToExponential
        | N::NumberToPrecision
        | N::NumberToString
        | N::DataViewGet(_)
        | N::DataViewSet(_)
        | N::DataViewGetBig { .. }
        // `ToIndex` of the bit count, which is a `ToNumber` first: `BigInt.asIntN(obj, x)` has
        // to call `obj.valueOf()`. The *second* argument is deliberately not here -- it is a
        // BigInt, and coercing it to a number would be the one conversion the language refuses.
        | N::BigIntAsIntN
        | N::BigIntAsUintN
        | N::DataViewSetBig { .. } => &[0],
        _ => &[],
    }
}

pub(super) fn is_reflect_target(value: &Value) -> bool {
    !matches!(
        value,
        Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::BigInt(_)
            | Value::Symbol(_)
            | Value::Cell(_)
    )
}

/// What a proxy ultimately wraps, following a chain of them.
///
/// For the operations that are defined on the *shape* of a value rather than on its properties
/// -- `Array.isArray`, `instanceof` -- and which therefore have no trap to consult.
pub(super) fn unwrap_proxy(value: &Value) -> &Value {
    let mut current = value;
    for _ in 0..MAX_PROTOTYPE_DEPTH {
        match current {
            Value::Proxy(proxy) => current = &proxy.target,
            other => return other,
        }
    }
    current
}

/// The built-in that constructs values of this kind, for `x.constructor`.
///
/// `None` for the kinds that answer through an ordinary prototype chain -- a plain object and a
/// compiled function both carry `constructor` as a property, and inventing one here would shadow
/// what the program put there.
pub(super) fn intrinsic_constructor(value: &Value) -> Option<Value> {
    let realm = value_realm(value);
    let native = match value {
        // A `Temporal` value's constructor is the one of the eight its kind names, which is what
        // `date.constructor === Temporal.PlainDate` asks and what a library reaches for to build
        // another of the same type.
        Value::Temporal(built) => Native::Temporal(temporal::Op::Constructor(built.kind())),
        Value::Date(_) => Native::DateConstructor,
        Value::RegExp(_) => Native::RegExpConstructor,
        Value::Promise(_) => Native::PromiseConstructor,
        Value::Array(_) => Native::ArrayConstructor,
        Value::String(_) => Native::StringConstructor,
        Value::Number(_) => Native::NumberConstructor,
        Value::Boolean(_) => Native::BooleanConstructor,
        Value::BigInt(_) => Native::BigIntConstructor,
        Value::Symbol(_) => Native::SymbolConstructor,
        _ => return None,
    };
    Some(Value::Native(native, realm))
}

/// `call`, `apply`, and `bind`, which every callable has and nothing else does.
///
/// Returned unbound, like every other method here: the receiver is the function being called
/// through, and `CallMethod` supplies it.
pub(super) fn function_property(intrinsics: &Intrinsics, name: &str, realm: RealmId) -> Value {
    // `Function.prototype` owns its constructor. A compiled or bound function reaches that
    // table before `Object.prototype`, so answering Object here is observably wrong even though
    // both values are constructors (`fn.constructor.name` is ecosystem feature detection).
    if name == "constructor" {
        return Value::Native(Native::FunctionConstructor, realm);
    }
    match table_step(
        intrinsics,
        PrototypeTable::Function,
        name,
        tabulated_function_property(name),
        realm,
    ) {
        // A function is an object, so it inherits `Object.prototype`'s methods too --
        // `Object.hasOwnProperty.call(x, k)` reads them off the *constructor*, which is how the
        // plain-object check every library ships is written.
        Value::Undefined => inherited_object_property(intrinsics, name, realm),
        found => found,
    }
}

pub(super) fn function_method_value(
    method: kiln_builtin_surface::FunctionMethod,
    realm: RealmId,
) -> Value {
    match method {
        kiln_builtin_surface::FunctionMethod::Apply => Value::Native(Native::FunctionApply, realm),
        kiln_builtin_surface::FunctionMethod::Bind => Value::Native(Native::FunctionBind, realm),
        kiln_builtin_surface::FunctionMethod::Call => Value::Native(Native::FunctionCall, realm),
        kiln_builtin_surface::FunctionMethod::ToString => {
            Value::Native(Native::FunctionToString, realm)
        }
    }
}

/// The same three with the realm's store left out: what `Function.prototype` was *built* with.
pub(super) fn tabulated_function_property(name: &str) -> Value {
    let Some(member) = kiln_builtin_surface::installed_prototype_member(
        kiln_builtin_surface::PrototypeId::Function,
        name,
    ) else {
        return Value::Undefined;
    };
    match member.binding {
        kiln_builtin_surface::SemanticBinding::FunctionMethod(method) => {
            function_method_value(method, RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::FunctionHasInstance => {
            Value::Native(Native::FunctionHasInstance, RealmId::PRIMARY)
        }
        _ => Value::Undefined,
    }
}

pub(super) fn map_method_value(method: kiln_builtin_surface::MapMethod, realm: RealmId) -> Value {
    match method {
        kiln_builtin_surface::MapMethod::Clear => {
            Value::Native(Native::CollectionClear(true), realm)
        }
        kiln_builtin_surface::MapMethod::Delete => Value::Native(Native::MapDelete, realm),
        kiln_builtin_surface::MapMethod::Entries => {
            Value::Native(Native::CollectionEntries(true), realm)
        }
        kiln_builtin_surface::MapMethod::ForEach => {
            Value::Native(Native::CollectionForEach(true), realm)
        }
        kiln_builtin_surface::MapMethod::Get => Value::Native(Native::MapGet, realm),
        kiln_builtin_surface::MapMethod::Has => Value::Native(Native::MapHas, realm),
        kiln_builtin_surface::MapMethod::Keys => Value::Native(Native::CollectionKeys(true), realm),
        kiln_builtin_surface::MapMethod::Set => Value::Native(Native::MapSet, realm),
        kiln_builtin_surface::MapMethod::Values => {
            Value::Native(Native::CollectionValues(true), realm)
        }
    }
}

pub(super) fn tabulated_map_property(name: &str) -> Value {
    let Some(member) = kiln_builtin_surface::installed_prototype_member(
        kiln_builtin_surface::PrototypeId::Map,
        name,
    ) else {
        return Value::Undefined;
    };
    match member.binding {
        kiln_builtin_surface::SemanticBinding::Constructor => {
            Value::Native(Native::MapConstructor, RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::MapMethod(method) => {
            map_method_value(method, RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::CollectionIterator(true) => {
            Value::Native(Native::CollectionEntries(true), RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::StringTag(tag) => {
            Value::String(tag.to_owned().into())
        }
        _ => Value::Undefined,
    }
}

pub(super) fn set_method_value(method: kiln_builtin_surface::SetMethod, realm: RealmId) -> Value {
    match method {
        kiln_builtin_surface::SetMethod::Add => Value::Native(Native::SetAdd, realm),
        kiln_builtin_surface::SetMethod::Clear => {
            Value::Native(Native::CollectionClear(false), realm)
        }
        kiln_builtin_surface::SetMethod::Delete => Value::Native(Native::SetDelete, realm),
        kiln_builtin_surface::SetMethod::Difference => {
            Value::Native(Native::SetOperation(SetOperation::Difference), realm)
        }
        kiln_builtin_surface::SetMethod::Entries => {
            Value::Native(Native::CollectionEntries(false), realm)
        }
        kiln_builtin_surface::SetMethod::ForEach => {
            Value::Native(Native::CollectionForEach(false), realm)
        }
        kiln_builtin_surface::SetMethod::Has => Value::Native(Native::SetHas, realm),
        kiln_builtin_surface::SetMethod::Intersection => {
            Value::Native(Native::SetOperation(SetOperation::Intersection), realm)
        }
        kiln_builtin_surface::SetMethod::IsDisjointFrom => {
            Value::Native(Native::SetOperation(SetOperation::IsDisjointFrom), realm)
        }
        kiln_builtin_surface::SetMethod::IsSubsetOf => {
            Value::Native(Native::SetOperation(SetOperation::IsSubsetOf), realm)
        }
        kiln_builtin_surface::SetMethod::IsSupersetOf => {
            Value::Native(Native::SetOperation(SetOperation::IsSupersetOf), realm)
        }
        kiln_builtin_surface::SetMethod::Keys => {
            Value::Native(Native::CollectionValues(false), realm)
        }
        kiln_builtin_surface::SetMethod::SymmetricDifference => Value::Native(
            Native::SetOperation(SetOperation::SymmetricDifference),
            realm,
        ),
        kiln_builtin_surface::SetMethod::Union => {
            Value::Native(Native::SetOperation(SetOperation::Union), realm)
        }
        kiln_builtin_surface::SetMethod::Values => {
            Value::Native(Native::CollectionValues(false), realm)
        }
    }
}

pub(super) fn tabulated_set_property(name: &str) -> Value {
    let Some(member) = kiln_builtin_surface::installed_prototype_member(
        kiln_builtin_surface::PrototypeId::Set,
        name,
    ) else {
        return Value::Undefined;
    };
    match member.binding {
        kiln_builtin_surface::SemanticBinding::Constructor => {
            Value::Native(Native::SetConstructor, RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::SetMethod(method) => {
            set_method_value(method, RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::CollectionIterator(false) => {
            Value::Native(Native::CollectionValues(false), RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::StringTag(tag) => {
            Value::String(tag.to_owned().into())
        }
        _ => Value::Undefined,
    }
}

pub(super) fn weak_map_method_value(
    method: kiln_builtin_surface::WeakMapMethod,
    realm: RealmId,
) -> Value {
    match method {
        kiln_builtin_surface::WeakMapMethod::Delete => Value::Native(Native::WeakMapDelete, realm),
        kiln_builtin_surface::WeakMapMethod::Get => Value::Native(Native::WeakMapGet, realm),
        kiln_builtin_surface::WeakMapMethod::GetOrInsert => Value::Native(
            Native::CollectionGetOrInsert {
                weak: true,
                computed: false,
            },
            realm,
        ),
        kiln_builtin_surface::WeakMapMethod::GetOrInsertComputed => Value::Native(
            Native::CollectionGetOrInsert {
                weak: true,
                computed: true,
            },
            realm,
        ),
        kiln_builtin_surface::WeakMapMethod::Has => Value::Native(Native::WeakMapHas, realm),
        kiln_builtin_surface::WeakMapMethod::Set => Value::Native(Native::WeakMapSet, realm),
    }
}

pub(super) fn tabulated_weak_map_property(name: &str) -> Value {
    let Some(member) = kiln_builtin_surface::installed_prototype_member(
        kiln_builtin_surface::PrototypeId::WeakMap,
        name,
    ) else {
        return Value::Undefined;
    };
    match member.binding {
        kiln_builtin_surface::SemanticBinding::Constructor => {
            Value::Native(Native::WeakMapConstructor, RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::WeakMapMethod(method) => {
            weak_map_method_value(method, RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::StringTag(tag) => {
            Value::String(tag.to_owned().into())
        }
        _ => Value::Undefined,
    }
}

pub(super) fn weak_set_method_value(
    method: kiln_builtin_surface::WeakSetMethod,
    realm: RealmId,
) -> Value {
    match method {
        kiln_builtin_surface::WeakSetMethod::Add => Value::Native(Native::WeakSetAdd, realm),
        kiln_builtin_surface::WeakSetMethod::Delete => Value::Native(Native::WeakSetDelete, realm),
        kiln_builtin_surface::WeakSetMethod::Has => Value::Native(Native::WeakSetHas, realm),
    }
}

pub(super) fn tabulated_weak_set_property(name: &str) -> Value {
    let Some(member) = kiln_builtin_surface::installed_prototype_member(
        kiln_builtin_surface::PrototypeId::WeakSet,
        name,
    ) else {
        return Value::Undefined;
    };
    match member.binding {
        kiln_builtin_surface::SemanticBinding::Constructor => {
            Value::Native(Native::WeakSetConstructor, RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::WeakSetMethod(method) => {
            weak_set_method_value(method, RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::StringTag(tag) => {
            Value::String(tag.to_owned().into())
        }
        _ => Value::Undefined,
    }
}

/// The number a `Number.prototype` method is running on, or the `TypeError` it owes.
///
/// Not `as_number`: these three are the specification's `ThisNumberValue`, which accepts a number
/// and a Number object and *refuses* everything else rather than coercing it. Coercing was the
/// old behaviour and it made `Number.prototype.toFixed.call("1.005")` answer where it should
/// throw. A wrapper has already been opened by the time this runs.
pub(super) fn branded_number(receiver: &Value) -> Result<f64, ExecutionError> {
    match receiver {
        Value::Number(value) => Ok(*value),
        other => Err(ExecutionError::TypeErrorMessage {
            message: format!(
                "Number.prototype method called on {}",
                other.to_display_string()
            ),
        }),
    }
}

/// The boolean a `Boolean.prototype` method is running on, or the `TypeError` it owes.
pub(super) fn branded_boolean(receiver: &Value) -> Result<bool, ExecutionError> {
    match receiver {
        Value::Boolean(value) => Ok(*value),
        other => Err(ExecutionError::TypeErrorMessage {
            message: format!(
                "Boolean.prototype method called on {}",
                other.to_display_string()
            ),
        }),
    }
}

/// The characters a String wrapper owns, read off the box rather than through its prototype.
///
/// A String object is *exotic*: the specification gives it an own, non-configurable property per
/// character plus a `length`, which is what makes `Object.keys(new String("ab"))` `["0", "1"]`
/// and `delete new String("a")[0]` fail. Every other wrapper owns nothing.
pub(super) fn string_exotic(target: &Value, key: &str) -> Option<Value> {
    string_exotic_key(target, PropertyKeyRef::Utf8(key))
}

pub(super) fn string_exotic_key(target: &Value, key: PropertyKeyRef<'_>) -> Option<Value> {
    let Some(Value::String(text)) = wrapped_primitive(target) else {
        return None;
    };
    if key.matches_ascii("length") {
        return Some(Value::Number(text.unit_len() as f64));
    }
    let index = array_index_key(key)?;
    text.unit_at(index)
        .map(|unit| Value::String(StringUnits::from_units(&[unit])))
}

/// Whether this array still behaves ordinarily enough for the dense fast paths to answer for it.
///
/// An array whose `length` a program has locked is not one: `push` has to refuse rather than
/// grow it, `sort` has to refuse rather than rearrange it, and the refusal is a `TypeError` the
/// specification raises from inside the generic walk. The dense paths have no way to express
/// that, so such an array is walked the way any other object with a `length` is -- which is the
/// same algorithm, and already written.
pub(super) fn array_walks_densely(items: &ArrayData) -> bool {
    items.length_writable.get()
        && !items.sparse.get()
        && items.elements.borrow().is_empty()
        && items.properties.borrow().is_extensible()
}

/// The property an array holds at `index` outside its dense storage, if any.
pub(super) fn array_exotic_element(items: &ArrayData, index: usize) -> Option<ExoticElement> {
    items
        .elements
        .borrow()
        .iter()
        .find(|entry| entry.index == index)
        .cloned()
}

/// What an index reaches on an array's prototype chain, for a position the array has no element
/// at.
///
/// `Array.prototype[1] = 1` really does make `[0, , 2][1]` read as 1 and `1 in [0, , 2]` true: a
/// hole is the *absence* of an own property rather than a stored undefined, so the read carries
/// on up the chain exactly as it would for any other missing name. Answering `undefined` for one
/// made every generic walk visit a position no engine visits, and skip one every engine does.
pub(super) fn array_inherited_index(
    intrinsics: &Intrinsics,
    items: &ArrayData,
    name: &str,
) -> Option<Property> {
    array_inherited_index_key(intrinsics, items, PropertyKeyRef::Utf8(name))
}

pub(super) fn array_inherited_index_key(
    intrinsics: &Intrinsics,
    items: &ArrayData,
    key: PropertyKeyRef<'_>,
) -> Option<Property> {
    if let Some(found) = items.properties.borrow().lookup_key(key) {
        return Some(found);
    }
    // An array's indexed lookup starts at its actual realm's Array.prototype. Continue along the
    // links as they stand now: assuming Object.prototype is next invents inherited indices after
    // the program has replaced Array.prototype's parent with a null-prototype object.
    if items.properties.borrow().prototype().is_some() {
        return None;
    }
    let mut current = Value::Native(Native::PrototypeOf(PrototypeTable::Array), RealmId::PRIMARY);
    for _ in 0..MAX_PROTOTYPE_DEPTH {
        // A proxy in this chain needs the VM's observable [[Get]]/[[HasProperty]] path. Returning
        // no stored property lets that path preserve traps and the original array receiver.
        if matches!(current, Value::Proxy(_)) {
            return None;
        }
        if let Some((property, _)) = own_property_state_key(intrinsics, &current, key) {
            return Some(property);
        }
        current = prototype_of_in(intrinsics, &current);
        if matches!(current, Value::Null | Value::Undefined) {
            return None;
        }
    }
    None
}

/// The dense storage behind an array receiver, when the fast paths may answer for it.
pub(super) fn dense_array(value: &Value) -> Option<&Rc<ArrayData>> {
    match value {
        Value::Array(items) if array_walks_densely(items) => Some(items),
        _ => None,
    }
}

/// What an array answers as an own property: an element, its `length`, or nothing.
///
/// Separate from the property store beside it because neither of those two is in the store --
/// the elements are a dense `Vec` and `length` is derived from its size -- and a prototype chain
/// that walks into an array has to see all three.
/// The first link a *read* of `target` walks to.
///
/// [`prototype_of`] is the reflective chain and answers `Null` for a primitive, which is what
/// `Object.getPrototypeOf` needs before it coerces. A read does not coerce: `"s".at` finds
/// `String.prototype.at` with no wrapper made anywhere, so the chain a read walks starts at the
/// wrapper prototype rather than at nothing. Reading the two off one function is what left
/// strings and numbers behind when every other kind could see an accessor a program had defined.
pub(super) fn read_chain_start(target: &Value) -> Value {
    let realm = value_realm(target);
    let table = match target {
        Value::String(_) => PrototypeTable::String,
        Value::Number(_) => PrototypeTable::Number,
        Value::Boolean(_) => PrototypeTable::Boolean,
        Value::BigInt(_) => PrototypeTable::BigInt,
        Value::Symbol(_) => PrototypeTable::Symbol,
        _ => return prototype_of(target),
    };
    Value::Native(Native::PrototypeOf(table), realm)
}

/// An accessor a *built-in prototype* carries, for a value whose chain reaches one.
///
/// Nothing links to a built-in prototype. The tables are match arms rather than stores, and
/// `ObjectData::lookup` walks stored links and stops at the last one, so the realm's record of
/// what a program defined on `Array.prototype` is reached by name and by nothing else. A *data*
/// property defined there is still answered, because the read falls through to the table and the
/// table consults the realm's store on the way past. An accessor cannot be: it is a call, and
/// reaching it needs the `Property` itself rather than the value it would have read as.
///
/// Without this, `Object.defineProperty(Array.prototype, "x", { get })` is visible on the
/// prototype and invisible through every array -- and that asymmetry is not a curiosity. It is
/// how a redefined `constructor` read as the table's own: species construction asks an instance
/// for `.constructor`, a program that installs one installs an accessor, and the instance
/// answered `Array` while the prototype answered the override.
///
/// The walk stops at the first link that answers `name` at all -- an override holding a data
/// property, or a table publishing the name itself -- because carrying on past it would let an
/// accessor further up shadow the thing that actually answers, which is the direction a prototype
/// chain does not run in.
pub(super) fn inherited_intrinsic_accessor_key(
    intrinsics: &Intrinsics,
    target: &Value,
    key: PropertyKeyRef<'_>,
) -> Option<Property> {
    // The common case by an enormous margin: no program has defined an accessor on any built-in,
    // so there is nothing up there to find and every read that misses its own store pays one
    // load to learn it. This walk is otherwise on the path of `array.map`.
    if !intrinsics.any_accessor() {
        return None;
    }
    let mut link = read_chain_start(target);
    // Table accessors are named with ASCII spellings. An unknown units key cannot name one, and
    // must remain on the exact store path rather than being converted through replacement chars.
    let Some(table_name) = key.known_ascii() else {
        return None;
    };
    let table_name = table_name.as_ref();
    // Bounded rather than trusted. `Object.setPrototypeOf` refuses to build a cycle, but this
    // walks the chain a value *kind* gives as well as the stored one, and a bounded walk cannot
    // hang whatever the two combine into. Real chains are a handful of links deep.
    for _ in 0..64 {
        let Value::Native(native, _) = link else {
            if matches!(link, Value::Null | Value::Undefined) {
                return None;
            }
            // An ordinary object on the chain has already been walked by `lookup`; carry on so
            // that a moved chain -- `array.__proto__ = {}` -- still reaches the table that
            // object's own prototype ends at.
            link = prototype_of(&link);
            continue;
        };
        match intrinsics.overridden_key(&link, key) {
            Overridden::Held(property @ Property::Accessor { .. }, _) => return Some(property),
            // A data property answers here, and the ordinary read is what finds it.
            Overridden::Held(Property::Value(_), _) => return None,
            // Deleted here, so the chain carries on past it -- which is the whole point of the
            // tombstone.
            Overridden::Absent => {}
            Overridden::Untouched => {
                if intrinsic_owns(native, &table_name).is_some()
                    || intrinsic_accessor(&link, &table_name).is_some()
                    || machine_accessor(&link, &table_name).is_some()
                {
                    return None;
                }
            }
        }
        link = prototype_of(&link);
    }
    None
}

pub(super) fn array_own_property_key(
    items: &ArrayData,
    key: PropertyKeyRef<'_>,
) -> Option<Property> {
    if key.matches_ascii("length") {
        #[expect(clippy::cast_precision_loss)]
        let length = items.length() as f64;
        return Some(Property::Value(Value::Number(length)));
    }
    let index = array_index_key(key)?;
    items
        .borrow()
        .get(index)
        .filter(|value| !matches!(value, Value::Native(Native::Hole, _)))
        .cloned()
        .map(Property::Value)
}

/// The primitive a wrapper object boxes, or `None` for anything else.
///
/// The three wrapper *prototypes* are themselves wrappers: the specification gives
/// `Number.prototype` a `[[NumberData]]` of +0, `String.prototype` a `[[StringData]]` of "" and
/// `Boolean.prototype` a `[[BooleanData]]` of false. That is not a curiosity -- it is what makes
/// `Number.prototype.valueOf()` answer 0 instead of throwing, which a program reaches by calling
/// any of these methods without a receiver of its own.
/// The two functions a `NewPromiseCapability(C)` is waiting for.
///
/// Shared between the executor `C` is called with and the caller that will use them, because the
/// executor is *how* they arrive: the specification's own way of saying "whatever `C` does with
/// its argument, these are the functions that settle the promise it returns".
pub struct CapabilityRecord {
    pub(super) resolve: RefCell<Value>,
    pub(super) reject: RefCell<Value>,
}

impl std::fmt::Debug for CapabilityRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CapabilityRecord")
    }
}

impl Default for CapabilityRecord {
    fn default() -> Self {
        // Undefined until the executor is called, which is exactly the test the specification
        // makes: a constructor that never calls it leaves both undefined, and that is the
        // `TypeError` rather than a promise nothing holds the settlers for.
        Self {
            resolve: RefCell::new(Value::Undefined),
            reject: RefCell::new(Value::Undefined),
        }
    }
}

pub(super) fn wrapped_primitive(value: &Value) -> Option<Value> {
    match value {
        Value::Object(object) => object.borrow().primitive().cloned(),
        Value::Native(Native::PrototypeOf(PrototypeTable::Number), _) => Some(Value::Number(0.0)),
        Value::Native(Native::PrototypeOf(PrototypeTable::String), _) => {
            Some(Value::String(String::new().into()))
        }
        Value::Native(Native::PrototypeOf(PrototypeTable::Boolean), _) => {
            Some(Value::Boolean(false))
        }
        _ => None,
    }
}

/// `ToObject(value)`: the object a value stands for, or `None` for the two that stand for none.
///
/// Every `Object` static and every `Object.prototype` method begins with this step, and skipping
/// it is visible twice over: `Object.assign(1, { a: 2 })` raised "cannot set a property on a
/// number" where the specification boxes the number and copies onto the box, and
/// `Object.prototype.hasOwnProperty.call(undefined, "x")` answered `false` where it owes a
/// `TypeError` -- an answer a feature-detect reads as "no such property".
pub(crate) fn to_object(value: &Value) -> Option<Value> {
    match value {
        Value::Undefined | Value::Null => None,
        Value::Boolean(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::Symbol(_)
        | Value::BigInt(_) => Some(wrap_primitive(value.clone())),
        other => Some(other.clone()),
    }
}

/// The characters a String wrapper owns, as enumerable own properties.
///
/// They are held in the boxed string rather than in the store beside it, so every walk over own
/// properties has to ask for them separately. `Object.keys(Object("abc"))` was `[]` while
/// `Object.getOwnPropertyNames` of the same object listed all three, which is the two walks
/// disagreeing about one object.
pub(super) fn string_wrapper_entries_units(value: &Value) -> Vec<(StringUnits, Property)> {
    let Some(Value::String(text)) = wrapped_primitive(value) else {
        return Vec::new();
    };
    text.units()
        .enumerate()
        .map(|(index, unit)| {
            (
                StringUnits::from(index.to_string()),
                Property::Value(Value::String(StringUnits::from_units(&[unit]))),
            )
        })
        .collect()
}

/// `new String("x")`, `new Number(1)`, `new Boolean(true)`, and the `Object(primitive)` that
/// builds the same thing.
///
/// The prototype is the matching wrapper table, which is what makes `new Boolean(true)` answer
/// `instanceof Boolean` and reach `Boolean.prototype.valueOf` -- the two observations that make a
/// wrapper worth having at all rather than an empty object with a value hidden inside it.
pub(super) fn wrap_primitive(primitive: Value) -> Value {
    let table = match primitive {
        Value::String(_) => PrototypeTable::String,
        Value::Number(_) => PrototypeTable::Number,
        Value::Boolean(_) => PrototypeTable::Boolean,
        Value::Symbol(_) => PrototypeTable::Symbol,
        Value::BigInt(_) => PrototypeTable::BigInt,
        // Nothing else is boxable: `Object(x)` passes an object through and refuses null and
        // undefined before it reaches here.
        _ => PrototypeTable::Object,
    };
    let mut data = ObjectData::default();
    data.set_prototype(Value::Native(Native::PrototypeOf(table), RealmId::PRIMARY));
    data.primitive = Some(primitive);
    Value::Object(tracked_object(data))
}

/// The receiver a built-in sees, with a wrapper opened.
///
/// `String.prototype.charAt.call(new String("ab"), 0)` has to read the boxed string, and so does
/// every other method on a wrapper prototype -- there are fifty of them and each would otherwise
/// need the same unwrapping written out again. The generic object methods are deliberately not
/// in the set: `Object.prototype.toString` reports the *box*, which is the one thing that can
/// tell a wrapper apart from what it wraps.
pub(super) fn receiver_for(native: Native, receiver: &Value) -> Option<Value> {
    let boxed = wrapped_primitive(receiver)?;
    let opens = match native {
        // `Object.prototype.valueOf` is deliberately absent: it answers with the box.
        Native::PrimitiveToString => matches!(boxed, Value::Symbol(_)),
        Native::SymbolValueOf => matches!(boxed, Value::Symbol(_)),
        Native::StringValueOf | Native::StringToStringMethod => {
            matches!(boxed, Value::String(_))
        }
        Native::BigIntValueOf | Native::BigIntToString => matches!(boxed, Value::BigInt(_)),
        Native::NumberToString
        | Native::NumberToFixed
        | Native::NumberToExponential
        | Native::NumberToPrecision
        | Native::NumberValueOf => true,
        Native::BooleanToString | Native::BooleanValueOf => true,
        // Generic String prototype methods must see the wrapper, not its primitive payload:
        // abstract ToString then observes an own @@toPrimitive/toString override. Only the
        // brand-checked valueOf/toString methods above open the wrapper directly.
        other if other.walks_a_string() => false,
        // match/matchAll/search/replace/split first delegate through their RegExp symbol methods
        // when present. That protocol receives the original this value, so these must not open
        // a String wrapper before the VM has performed the protocol lookup.
        Native::StringMatch | Native::StringMatchAll | Native::StringSearch => false,
        // Static String methods ignore their receiver, so opening a wrapper here is harmless.
        other => format!("{other:?}").starts_with("String"),
    };
    opens.then_some(boxed)
}

/// `f.name` and `f.length` read off a built-in, which keeps them in no store.
pub(super) fn identity_property(target: &Value, key: &str) -> Option<Value> {
    let (name, length) = intrinsic_identity(target)?;
    match key {
        "name" => Some(Value::String(name.into())),
        "length" => Some(Value::Number(length)),
        _ => None,
    }
}

/// Whether a compiled function has a `prototype` property at all.
///
/// An arrow has none: it cannot be constructed, so there is no object for instances to link to
/// and the specification gives it no slot. Listing one anyway would put a key in
/// `Object.getOwnPropertyNames` that reading it has to invent.
/// Whether this function carries the own `prototype` property that `new` would use.
///
/// Not every callable does. An arrow has never had one here; an *async* function should not
/// either, and did -- `(async function () {}).hasOwnProperty("prototype")` answered true where
/// every other engine answers false. An async generator is the exception that looks like a
/// contradiction: it keeps a `prototype`, because that object is what its generator instances
/// inherit from, not something `new` reaches.
///
/// A *method* -- shorthand, class, static, getter or setter -- has none unless it is a generator
/// (generators keep `prototype` because generator instances inherit from it).
pub(super) fn has_prototype_property(closure: &Closure) -> bool {
    closure
        .program
        .functions
        .get(closure.function as usize)
        .is_some_and(|proto| {
            !proto.arrow
                && (!proto.asynchronous || proto.async_generator)
                && (!proto.is_method || proto.generator)
        })
}

/// The attributes every function's own `name` and `length` carry.
///
/// Locked against assignment and hidden from enumeration, but *configurable*: the specification
/// makes these the one pair a program is expected to redefine -- which is how a decorator
/// renames a function without replacing it.
/// `SetFunctionName(function, key, prefix)`.
///
/// The own private element under this reserved key, with the flags its slot carries.
///
/// Own-only on purpose: the presence of the name on the object *is* the brand check, so a
/// prototype walk would hand one class's private member to another class's instance.
pub(super) fn own_private_element(target: &Value, key: &str) -> Option<(Property, Attributes)> {
    let store = prototype_slot(target)?;
    let data = store.borrow();
    Some((
        data.get_own_property(key)?.clone(),
        data.get_own_attributes(key)?,
    ))
}

/// A private name as the program spelled it, without the module-and-scope reservation it is keyed
/// under: `x` from `@@private:<module-scope>:x`.
pub(super) fn private_display_name(key: &str) -> &str {
    let reserved = key.strip_prefix(PRIVATE_KEY_PREFIX).unwrap_or(key);
    reserved.split_once(':').map_or(reserved, |(_, name)| name)
}

/// A method, a getter and a setter are each created anonymous and named by the definition that
/// installs them, which is also the only place a computed key is known. Nothing did it, so
/// `class C { m() {} }` left `C.prototype.m.name` empty, `{ get x() {} }` reported `""` where the
/// language says `"get x"`, and a symbol-keyed method reported nothing at all. That name is not
/// decoration: a decorator reads it, a stack trace prints it, and Test262 asserts it on almost
/// every method it defines.
pub(super) fn define_function_name(
    function: &Value,
    key: &Value,
    prefix: Option<&str>,
) -> Result<(), ExecutionError> {
    let Value::Function(closure) = function else {
        return Ok(());
    };
    let name = match key {
        // A symbol names the property after its description, in brackets; one with no description
        // names it nothing, and a prefix is still applied to that nothing.
        Value::Symbol(symbol) => match symbol.description_units() {
            None => StringUnits::default(),
            Some(description) => {
                let mut builder = StringUnitsBuilder::new();
                builder.push_ascii("[")?;
                builder.push_units(&description)?;
                builder.push_ascii("]")?;
                builder.finish()
            }
        },
        other => {
            let key = other.property_key_units_checked()?;
            // A private name is spelled the way the program wrote it rather than the way the
            // store keys it: `#m`, not the scope-numbered reservation standing in for it.
            match key.to_scalar_string() {
                Ok(scalar_key) => match scalar_key.strip_prefix(PRIVATE_KEY_PREFIX) {
                    Some(reserved) => {
                        let display = reserved.split_once(':').map_or(reserved, |(_, name)| name);
                        let mut builder = StringUnitsBuilder::new();
                        builder.push_ascii("#")?;
                        builder.push_units(&StringUnits::from(display))?;
                        builder.finish()
                    }
                    None => key,
                },
                // Ordinary property keys can contain lone surrogates. They are already in the
                // canonical units representation; keep them intact rather than widening through
                // a replacement character just to inspect a private-name prefix.
                Err(_) => key,
            }
        }
    };

    let name = prefix.map_or(Ok(name.clone()), |prefix| {
        let mut builder = StringUnitsBuilder::new();
        builder.push_ascii(prefix)?;
        builder.push_ascii(" ")?;
        builder.push_units(&name)?;
        Ok(builder.finish())
    })?;
    closure
        .properties
        .borrow_mut()
        .define_value("name", Value::String(name), FUNCTION_IDENTITY);
    Ok(())
}

pub(super) const FUNCTION_IDENTITY: Attributes = Attributes {
    writable: false,
    enumerable: false,
    configurable: true,
};

/// The `name` and `length` a runtime-supplied callable owns, or `None` for something that is not
/// a function at all.
///
/// `length` is the specification's declared parameter count, not this implementation's: it is
/// the number of arguments before the first optional one, so `Array.prototype.slice` is 2 and
/// `push` is 1 even though both accept any number. A program reads it to decide how many
/// arguments to forward, so guessing it wrong is worse than not answering.
pub(super) fn intrinsic_identity(value: &Value) -> Option<(String, f64)> {
    match value {
        Value::Native(native, _) => {
            // A namespace is an object, not a function: `Math` has no call behaviour, so it has
            // no arity and no name of its own either. A Node builtin module is a namespace for
            // the same reason -- `require("path")` is an object, and node's has neither a
            // `length` nor a `name` for `Object.getOwnPropertyNames` to report.
            if namespace_name(native).is_some() || node_builtin_specifier(*native).is_some() {
                return None;
            }
            let length = native_length(*native)?;
            // `Function.prototype` is a function that takes nothing, returns undefined and is
            // *anonymous*: the derived spelling would call it `prototypeOf`, which is the name
            // of the variant rather than of anything a program can see.
            // `%ThrowTypeError%` is anonymous by definition, and the derived spelling would
            // hand a program the name of a Rust variant.
            if matches!(native, Native::PrototypeOf(_) | Native::ThrowTypeError) {
                return Some((String::new(), length));
            }
            Some((native_js_name(native), length))
        }
        // An array method the machine drives rather than a free function, or a generator's own
        // `next`. Every one of them declares exactly one argument -- the callback, or the value
        // resumed with -- so there is nothing to tabulate.
        Value::Callback(method, _) => Some((builtin_js_name(&format!("{method:?}")), 1.0)),
        // `bind` reports the arity still to be supplied and prefixes the name it wraps, which is
        // what makes a stack trace through one readable.
        Value::Bound(bound) => {
            let (name, length) = function_identity(&bound.target)?;
            Some((
                format!("bound {name}"),
                (length - bound.arguments.len() as f64).max(0.0),
            ))
        }
        // A promise's `resolve`/`reject` pair: anonymous, one argument, exactly as the
        // specification builds them.
        Value::Settler(..) => Some((String::new(), 1.0)),
        // The `GetCapabilitiesExecutor` takes the pair, so its `length` is 2. Anonymous like the
        // settlers, and for the same reason: the specification gives it no name.
        Value::Capability(_) => Some((String::new(), 2.0)),
        // A combinator's resolve or reject element function: one argument, and anonymous. The
        // specification is explicit that these are anonymous built-ins, and test262 checks the
        // descriptor rather than the value -- so this has to be an own, non-writable,
        // non-enumerable, configurable property, which is what this table produces.
        Value::Element(_) => Some((String::new(), 1.0)),
        _ => None,
    }
}

/// The `name` and `length` of any callable, compiled or built in.
pub(super) fn function_identity(value: &Value) -> Option<(String, f64)> {
    match value {
        // Read from the store rather than from the closure's declared name, so that a function
        // whose `name` a program deleted reports what a read of it now reports -- the empty
        // string the specification substitutes for a target with no name of its own.
        Value::Function(closure) => {
            let properties = closure.properties.borrow();
            Some((
                match properties.get_own("name") {
                    Some(Value::String(name)) => name.to_scalar_string().unwrap_or_default(),
                    _ => String::new(),
                },
                match properties.get_own("length") {
                    Some(Value::Number(length)) => *length,
                    _ => 0.0,
                },
            ))
        }
        other => intrinsic_identity(other),
    }
}

/// The declared parameter count of one built-in, or `None` for something that is not callable.
///
/// Tabulated rather than derived: the number comes from the specification's own signature for
/// each function, and this implementation's Rust signature knows nothing about it -- every one
/// of these takes a slice. Grouped by arity so the table reads as the specification's tables do
/// rather than as two hundred separate claims.
#[expect(
    clippy::match_same_arms,
    reason = "grouped by arity, not by implementation"
)]
pub(super) const fn native_length(native: Native) -> Option<f64> {
    if let Native::AtomicsOp(op) = native {
        return Some(op.arity());
    }
    if let Native::HostAgent(op) = native {
        return Some(op.arity());
    }
    use Native as N;
    Some(match native {
        // Not functions at all: internal markers that never reach a program, and the method
        // tables, which are ordinary objects. `Function.prototype` is the exception -- it *is*
        // callable, and it is the anonymous function of no arguments that returns undefined.
        N::Hole | N::Uninitialized | N::ReturnSignal | N::Tombstone => return None,
        N::PrototypeOf(PrototypeTable::Function) => 0.0,
        // `OrdinaryHasInstance(C, O)` takes the one value being tested; the constructor is the
        // receiver.
        N::FunctionHasInstance => 1.0,
        N::SymbolToPrimitive => 1.0,
        N::TypedArrayToStringTag => 0.0,
        N::PrototypeOf(_) => return None,
        N::Math | N::Json | N::Reflect | N::Performance => return None,
        N::HostCreateRealm => 0.0,
        N::HostEvalScript => 1.0,
        N::ShadowRealmConstructor => 0.0,
        N::ShadowRealmEvaluate => 1.0,
        N::ShadowRealmImportValue => 2.0,
        // `Intl`'s arities come from its own table: the namespace and the six prototypes are
        // not callable, and the constructors do not all take the same number of arguments.
        N::Intl(op) => match intl::length(op) {
            Some(length) => length,
            None => return None,
        },
        // `Temporal`'s the same way, and for one reason more: two of its types publish a
        // same-named method with different arities, so the table is keyed by the type as well as
        // by the method. Everything here fell to the `_ => 1.0` default below, which was right
        // for `from` and wrong for all fifty-four zero-argument readers and all ten that take a
        // pair.
        N::Temporal(op) => match temporal::length(op) {
            Some(length) => length,
            None => return None,
        },
        N::ObjectKeys => match kiln_builtin_surface::native_function_length("Object", "keys") {
            Some(length) => length as f64,
            None => 1.0,
        },
        N::QueueMicrotask | N::SetTimeout | N::SetInterval | N::ClearTimeout | N::ClearInterval
        | N::SetImmediate | N::ClearImmediate => {
            let member = match native {
                N::QueueMicrotask => "queueMicrotask",
                N::SetTimeout => "setTimeout",
                N::SetInterval => "setInterval",
                N::ClearTimeout => "clearTimeout",
                N::ClearInterval => "clearInterval",
                N::SetImmediate => "setImmediate",
                _ => "clearImmediate",
            };
            kiln_builtin_surface::native_function_length("globalThis", member)
                .expect("timer callable has a shared arity") as f64
        }
        N::ObjectGetOwnPropertyDescriptor => {
            match kiln_builtin_surface::native_function_length("Object", "getOwnPropertyDescriptor") {
                Some(length) => length as f64,
                None => 2.0,
            }
        }
        N::FunctionCall | N::FunctionApply | N::FunctionBind => {
            let member = match native {
                N::FunctionCall => "call",
                N::FunctionApply => "apply",
                _ => "bind",
            };
            match kiln_builtin_surface::native_function_length("Function.prototype", member) {
                Some(length) => length as f64,
                None => 1.0,
            }
        }

        // The zero-argument built-ins: every accessor, every `toString`, and the collection
        // constructors, which take their contents through an optional argument.
        N::RegExpAccessor(_)
        | N::BuiltinAccessor(_)
        | N::ProtoGetter
        | N::ThrowTypeError
        | N::PromiseWithResolvers
        | N::MathRandom
        | N::DateNow
        | N::PerformanceNow
        | N::NumberValueOf
        | N::StringValueOf
        | N::SymbolValueOf
        | N::BigIntValueOf
        | N::ObjectToLocaleString
        | N::ObjectValueOf
        | N::ObjectToStringTag
        | N::PrimitiveToString
        | N::PrimitiveValueOf
        | N::BooleanToString
        | N::BooleanValueOf
        | N::BigIntToString
        | N::RegExpToString
        | N::ErrorToString
        | N::FunctionToString
        | N::ArrayCopy(ArrayCopy::ToReversed)
        | N::ArrayJoinDefault
        | N::ArrayToLocaleString
        | N::ArrayPop
        | N::ArrayReverse
        | N::ArrayShift
        | N::ArrayEntries
        | N::ArrayKeys
        | N::ArrayValues
        | N::ArrayFlat
        | N::ArrayOf
        | N::StringTrim
        | N::StringTrimStart
        | N::StringTrimEnd
        | N::StringToUpperCase
        | N::StringToLowerCase
        | N::StringNormalize
        | N::StringValues
        | N::StringIteratorNext
        | N::ArrayIteratorNext
        | N::CollectionIteratorNext
        | N::CollectionKeys(_)
        | N::CollectionValues(_)
        | N::CollectionEntries(_)
        | N::CollectionClear(_)
        | N::IteratorSelf
        | N::IteratorTerminal(TerminalKind::ToArray)
        | N::IteratorWrapNext
        | N::IteratorHelperNext(_)
        | N::IteratorHelperReturn
        | N::IteratorConstructor
        | N::TypedArrayIntrinsic
        | N::TypedArrayOf(_)
        | N::SetConstructor
        | N::MapConstructor
        | N::WeakSetConstructor
        | N::WeakMapConstructor
        | N::SymbolConstructor
        | N::RegExpStringIteratorNext
        | N::WeakRefDeref
        | N::DisposableStackConstructor
        | N::DisposableStackMove
        | N::DisposableStackDispose
        | N::AsyncDisposableStackConstructor
        | N::AsyncDisposableStackMove
        | N::AsyncDisposableStackDisposeAsync
        | N::ProxyRevoke
        // `AbortController()`, `AbortSignal` (never constructed directly), `TextEncoder()` and
        // `TextDecoder([label[, options]])` declare no required argument; all four fell to the
        // one-argument default and the native backend, which tabulates them at zero, was right.
        | N::AbortControllerConstructor
        | N::AbortSignalConstructor
        | N::TextEncoderConstructor
        | N::TextDecoderConstructor
        | N::ConsoleLog
        | N::ConsoleInfo
        | N::ConsoleDebug
        | N::ConsoleDir
        | N::ConsoleWarn
        | N::ConsoleError
        | N::ConsoleAssert
        | N::ConsoleClear
        | N::ConsoleCount
        | N::ConsoleCountReset
        | N::ConsoleGroup
        | N::ConsoleGroupEnd
        | N::ConsoleGroupCollapsed
        | N::ConsoleTable
        | N::ConsoleTime
        | N::ConsoleTimeEnd
        | N::ConsoleTimeLog
        | N::ConsoleTrace
        | N::ConsoleDirxml
        | N::ConsoleTimeStamp
        | N::ConsoleProfile
        | N::ConsoleProfileEnd => 0.0,

        // The two-argument ones. `Object.assign` is 2 because the specification counts the
        // target and one source; the rest of the sources are the rest parameter, which does not
        // count.
        N::ObjectAssign
        | N::ObjectCreate
        | N::ObjectSetPrototypeOf
        | N::ObjectDefineProperties
        | N::ObjectHasOwn
        | N::ObjectIs
        | N::ReflectGet
        | N::ReflectHas
        | N::ReflectConstruct
        | N::ReflectSetPrototypeOf
        | N::ReflectGetOwnPropertyDescriptor
        | N::ReflectDeleteProperty
        | N::JsonParse
        | N::MathImul
        | N::MathAtan2
        | N::MathPow
        | N::MathHypot
        | N::MathMin
        | N::MathMax
        | N::RegExpSymbol(RegExpSymbol::Replace | RegExpSymbol::Split)
        | N::ArrayCopy(ArrayCopy::With)
        | N::ArrayCopy(ArrayCopy::ToSpliced)
        | N::DisposableStackAdopt
        | N::AsyncDisposableStackAdopt
        | N::CollectionGetOrInsert { .. }
        | N::ArraySlice
        | N::ArraySplice
        | N::ArrayCopyWithin
        | N::StringSlice
        | N::StringSplit
        | N::StringSubstring
        | N::StringSubstr
        | N::StringReplace
        | N::StringReplaceAll
        | N::ParseInt
        | N::RegExpConstructor
        | N::PromiseThen
        | N::ProxyConstructor
        | N::ProxyRevocable
        | N::ArrayBufferSlice
        | N::DataViewSet(_)
        | N::DataViewSetBig { .. }
        | N::MapSet
        | N::ObjectGroupBy
        | N::MapGroupBy
        | N::BigIntAsIntN
        | N::BigIntAsUintN
        | N::FinalizationRegistryRegister
        | N::ArrayBufferSliceToImmutable
        // `AggregateError(errors, message)`: the options bag is optional.
        | N::Error(ErrorKind::Aggregate)
        | N::WeakMapSet => 2.0,

        // The three-argument ones.
        N::ObjectDefineProperty
        | N::ReflectDefineProperty
        | N::ReflectSet
        | N::ReflectApply
        | N::JsonStringify
        // `SuppressedError(error, suppressed, message)`.
        | N::Error(ErrorKind::Suppressed)
        | N::TypedArrayConstructor(_) => 3.0,

        // `Date` and `Date.UTC` take year through millisecond.
        N::DateConstructor | N::DateUtc => 7.0,

        // Every method on `Date.prototype` carries its own, for the reason [`DateMethod`] gives.
        N::Date(method) => method.length(),

        // `toBase64` takes only an options bag and `toHex` takes nothing, so neither declares a
        // required argument; the four that read or fill from a string declare the string.
        // `transferToImmutable([newLength])` declares no required argument, which is how a
        // program feature-detects it apart from `transfer`.
        N::Uint8Text(ByteText::ToBase64 | ByteText::ToHex)
        | N::ArrayBufferTransferToImmutable => 0.0,
        N::Uint8Text(ByteText::SetFromBase64) => 1.0,

        // Everything left takes exactly one required argument.
        _ => 1.0,
    })
}

/// Which built-in the realm's property store is keyed by.
///
/// A built-in is a `Copy` marker rather than an allocation -- two reads of `Math.max` produce
/// the same value, which is what makes `Math.max === Math.max` true -- so the marker *is* the
/// identity, and comparing two of them compares the built-ins they name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IntrinsicKey {
    Native(Native, RealmId),
    Callback(CallbackMethod, RealmId),
}

/// The key a built-in is stored under, or `None` for a value that is not one.
///
/// `Bound` and `Settler` are deliberately absent. Each is an allocation with identity of its
/// own, so a realm-wide store keyed by one would keep every bound function a program mutated
/// alive until the run ended; they keep the read-only projection until they carry a store of
/// their own.
pub(super) fn intrinsic_key(target: &Value) -> Option<IntrinsicKey> {
    match target {
        Value::Native(native, realm) => Some(IntrinsicKey::Native(*native, *realm)),
        Value::Callback(method, realm) => Some(IntrinsicKey::Callback(*method, *realm)),
        _ => None,
    }
}

/// What the realm's store says about one own property of a built-in.
pub(super) enum Overridden {
    /// The store holds the property, and its slot is the whole answer.
    Held(Property, Attributes),
    /// The complete materialized store does not contain this key; absence is authoritative.
    Absent,
    /// The store says nothing about this name, so the tables answer.
    Untouched,
}

/// Retag a marker returned by a realm-blind intrinsic table before storing it.
///
/// Native and Callback values remain Copy markers, but every realm owns a distinct marker identity.
/// Ordinary stored objects/functions are left untouched because they already carry their own
/// identity and realm.
pub(super) fn retag_intrinsic_property(property: Property, realm: RealmId) -> Property {
    match property {
        Property::Value(value) => Property::Value(in_realm(value, realm)),
        Property::Accessor { get, set } => Property::Accessor {
            get: get.map(|value| in_realm(value, realm)),
            set: set.map(|value| in_realm(value, realm)),
        },
    }
}

pub(super) struct Intrinsics {
    /// A linear scan rather than a map: this is empty for a program that leaves the built-ins
    /// alone and holds a handful of entries for one that does not, which is well below the size
    /// where hashing every lookup pays for itself.
    pub(super) entries: RefCell<Vec<(IntrinsicKey, ObjectData)>>,
    /// Whether any program has defined an *accessor* on any built-in.
    ///
    /// A guard for [`inherited_intrinsic_accessor_key`], which is asked on every read that misses a
    /// value's own store -- which is every method call on an array, a string or a map. False
    /// until a program does it and never cleared afterwards: `delete` removing the last one is
    /// rare enough that re-scanning to clear the flag would cost more than the walk it saves,
    /// and a stale `true` is only ever slower, never wrong.
    accessors: std::cell::Cell<bool>,
    /// Monotonic across all realms: a numeric property on either default array prototype
    /// invalidates the strict indexed-write shortcut, even after that property is deleted.
    pub(super) indexed_array_prototype: std::cell::Cell<bool>,
    /// Monotonic guard for sparse array shortcuts after either indexed prototype link changes.
    /// This is cached because map/reduce ask once per callback, and walking every materialized
    /// intrinsic entry there turns a dense map into quadratic work.
    indexed_prototype_chain_custom: std::cell::Cell<bool>,
    /// Each realm's nine error prototypes, indexed by realm and then by kind.
    ///
    /// Here rather than on the machine because `Error.prototype` is an own property of `Error`
    /// and every path that reports own properties -- `getOwnPropertyDescriptor`,
    /// `hasOwnProperty`, `getOwnPropertyNames`, a computed read -- reaches those tables with an
    /// `&Intrinsics` and nothing else. Held on the machine, the object was reachable only from
    /// the one instruction that spells `Error.prototype` in source, so `Error["prototype"]` was
    /// `undefined` while `Error.prototype` was an object, and the descriptor every
    /// `verifyNotConfigurable(Error, "prototype")` reads did not exist.
    ///
    /// Interior mutability for the same reason the entries have it: the prototype is built on
    /// first use, and first use is now a `&self` read.
    error_prototypes: RefCell<Vec<[Option<Value>; ErrorKind::ALL.len()]>>,
    /// Shared TypeScript Error stack accessors and private brand token, owned by each realm.
    /// Module VMs for a realm share this table, while a newly allocated realm starts empty.
    error_stack_accessors: RefCell<Vec<Option<ErrorStackAccessors>>>,
    unscopables: RefCell<Vec<Option<Value>>>,
}

#[derive(Clone)]
pub(super) struct ErrorStackAccessors {
    pub(super) getter: Value,
    pub(super) setter: Value,
    pub(super) brand_token: Value,
}

impl Default for Intrinsics {
    /// One realm's worth of slots, because a machine always has the primary realm.
    fn default() -> Self {
        Self {
            entries: RefCell::default(),
            accessors: std::cell::Cell::new(false),
            indexed_array_prototype: std::cell::Cell::new(
                kiln_builtin_surface::PROTOTYPE_SURFACE
                    .iter()
                    .any(|member| {
                        matches!(
                            member.prototype,
                            kiln_builtin_surface::PrototypeId::Array
                                | kiln_builtin_surface::PrototypeId::Object
                        ) && array_index_key(PropertyKeyRef::Utf8(member.key.property_key()))
                            .is_some()
                    }),
            ),
            indexed_prototype_chain_custom: std::cell::Cell::new(false),
            error_prototypes: RefCell::new(vec![[const { None }; ErrorKind::ALL.len()]]),
            error_stack_accessors: RefCell::new(vec![None]),
            unscopables: RefCell::new(vec![None]),
        }
    }
}

impl Intrinsics {
    /// A materialized intrinsic may have an ordinary mutable [[Prototype]] link of its own.
    pub(super) fn prototype_override(&self, target: &Value) -> Option<Value> {
        let identity = intrinsic_key(target)?;
        self.entries
            .borrow()
            .iter()
            .find(|(held, _)| *held == identity)
            .and_then(|(_, data)| data.prototype().cloned())
    }

    /// Store a changed [[Prototype]] on the same per-realm record that owns intrinsic properties.
    pub(super) fn record_prototype(&self, target: &Value, prototype: Value) -> bool {
        let Some(identity) = intrinsic_key(target) else {
            return false;
        };
        let realm = match target {
            Value::Native(Native::PrototypeOf(_), realm) => *realm,
            _ => RealmId::PRIMARY,
        };
        self.materialize(target);
        let mut entries = self.entries.borrow_mut();
        Self::data(&mut entries, identity).set_prototype(prototype);
        let mut array_parent = None;
        let mut object_parent = None;
        for (key, data) in entries.iter() {
            match key {
                IntrinsicKey::Native(Native::PrototypeOf(PrototypeTable::Array), owner_realm)
                    if *owner_realm == realm =>
                {
                    array_parent = data.prototype()
                }
                IntrinsicKey::Native(Native::PrototypeOf(PrototypeTable::Object), owner_realm)
                    if *owner_realm == realm =>
                {
                    object_parent = data.prototype()
                }
                _ => {}
            }
        }
        let default_array_parent = array_parent.is_none_or(|parent| {
            matches!(
                parent,
                Value::Native(
                    Native::PrototypeOf(PrototypeTable::Object),
                    owner_realm
                ) if *owner_realm == realm
            )
        });
        let default_object_parent =
            object_parent.is_none_or(|parent| matches!(parent, Value::Null));
        self.indexed_prototype_chain_custom
            .set(!default_array_parent || !default_object_parent);
        true
    }

    /// `realm`'s prototype for one error kind, built on first use.
    ///
    /// Per realm rather than per execution, because `e instanceof TypeError` compares it by
    /// identity: an error thrown by realm 1's built-in is an instance of *its* `TypeError` and
    /// not of the one the primary realm hands out, and one shared object would make both
    /// answers wrong at once.
    ///
    /// A subclass's prototype chains to `Error.prototype`, so `e instanceof Error` holds for
    /// every kind and `toString` is inherited rather than copied.
    pub(super) fn error_prototype(&self, kind: ErrorKind, realm: RealmId) -> Value {
        let index = realm.0 as usize;
        if let Some(prototype) = &self.error_prototypes.borrow()[index][kind.index()] {
            return prototype.clone();
        }
        let mut data = ObjectData::default();
        // None of these is enumerated: they are the realm's furniture, not something a program
        // put on the prototype, and `for (const k in e)` must not walk over them.
        data.set_non_enumerable("name", Value::String(kind.name().to_owned().into()));
        data.set_non_enumerable("message", Value::String(String::new().into()));
        // `e.constructor.name` is how a program asks which kind it caught, and this is the link
        // that answers it.
        data.set_non_enumerable("constructor", Value::Native(Native::Error(kind), realm));
        if matches!(kind, ErrorKind::Error) {
            // Marks the chain for `String(e)`. A real `toString` would be invoked through the
            // normal method path; this is the placeholder that keeps the observable behaviour
            // right until built-ins can be written in JavaScript (design.md §5).
            data.set_non_enumerable("toString", Value::Native(Native::ErrorToString, realm));
        }
        let prototype = Value::Object(tracked_object(data));
        if !matches!(kind, ErrorKind::Error) {
            // Recurses, so the borrow above has to be finished before this runs -- it is, and it
            // has to stay that way: building `TypeError.prototype` builds `Error.prototype`.
            let base = self.error_prototype(ErrorKind::Error, realm);
            if let Value::Object(object) = &prototype {
                object.borrow_mut().set_prototype(base);
            }
        }
        self.error_prototypes.borrow_mut()[index][kind.index()] = Some(prototype.clone());
        prototype
    }

    /// Make room for one more realm's error prototypes and unscopables.
    pub(super) fn add_realm(&self) {
        self.error_prototypes
            .borrow_mut()
            .push([const { None }; ErrorKind::ALL.len()]);
        self.error_stack_accessors.borrow_mut().push(None);
        self.unscopables.borrow_mut().push(None);
    }

    pub(super) fn error_stack_accessors(&self, realm: RealmId) -> Option<ErrorStackAccessors> {
        self.error_stack_accessors
            .borrow()
            .get(realm.0 as usize)
            .and_then(Clone::clone)
    }

    pub(super) fn register_error_stack_accessors(
        &self,
        realm: RealmId,
        accessors: ErrorStackAccessors,
    ) {
        let mut realms = self.error_stack_accessors.borrow_mut();
        if let Some(slot) = realms.get_mut(realm.0 as usize) {
            if slot.is_none() {
                *slot = Some(accessors);
            }
        }
    }

    pub(super) fn take_error_stack_accessors(&self) -> Vec<Value> {
        self.error_stack_accessors
            .borrow_mut()
            .iter_mut()
            .filter_map(Option::take)
            .flat_map(|accessors| [accessors.getter, accessors.setter, accessors.brand_token])
            .collect()
    }

    /// Every error prototype this store holds, taken out so the caller can break their cycles.
    pub(super) fn take_error_prototypes(&self) -> Vec<Value> {
        self.error_prototypes
            .borrow_mut()
            .iter_mut()
            .flat_map(|realm| realm.iter_mut().filter_map(Option::take))
            .collect()
    }

    /// Every unscopables object this store holds, taken out so the caller can break their cycles.
    pub(super) fn take_unscopables(&self) -> Vec<Value> {
        self.unscopables
            .borrow_mut()
            .iter_mut()
            .filter_map(Option::take)
            .collect()
    }

    /// `Array.prototype[Symbol.unscopables]` (ECMA-262 §23.1.3.32) for `realm`, built on first use.
    pub(super) fn array_unscopables(&self, realm: RealmId) -> Value {
        let index = realm.0 as usize;
        if let Some(unscopables) = self
            .unscopables
            .borrow()
            .get(index)
            .and_then(Option::as_ref)
        {
            return unscopables.clone();
        }
        let mut data = ObjectData::default();
        data.set_prototype(Value::Null);
        for name in [
            "at",
            "copyWithin",
            "entries",
            "fill",
            "find",
            "findIndex",
            "findLast",
            "findLastIndex",
            "flat",
            "flatMap",
            "includes",
            "keys",
            "toReversed",
            "toSorted",
            "toSpliced",
            "values",
        ] {
            data.set(name, Value::Boolean(true));
        }
        let unscopables = Value::Object(tracked_object(data));
        if index < self.unscopables.borrow().len() {
            self.unscopables.borrow_mut()[index] = Some(unscopables.clone());
        }
        unscopables
    }

    /// What the materialized store says about `key` on this built-in.
    pub(super) fn overridden(&self, target: &Value, key: &str) -> Overridden {
        self.overridden_key(target, PropertyKeyRef::Utf8(key))
    }

    pub(super) fn overridden_key(&self, target: &Value, key: PropertyKeyRef<'_>) -> Overridden {
        let Some(identity) = intrinsic_key(target) else {
            return Overridden::Untouched;
        };
        if matches!(
            target,
            Value::Native(
                Native::PrototypeOf(
                    PrototypeTable::Symbol
                        | PrototypeTable::Array
                        | PrototypeTable::Number
                        | PrototypeTable::Boolean
                        | PrototypeTable::BigInt
                        | PrototypeTable::Promise
                        | PrototypeTable::Object
                        | PrototypeTable::Function
                        | PrototypeTable::Map
                        | PrototypeTable::Set
                        | PrototypeTable::WeakMap
                        | PrototypeTable::WeakSet
                ),
                _
            )
        ) && !self.is_materialized(target)
        {
            self.materialize(target);
        }
        let entries = self.entries.borrow();
        let Some((_, slots)) = entries.iter().find(|(held, _)| *held == identity) else {
            return Overridden::Untouched;
        };
        let data = slots;
        if let Some(property) = data.get_own_property_key(key) {
            return Overridden::Held(
                property.clone(),
                data.get_own_attributes_key(key)
                    .unwrap_or(Attributes::DEFAULT),
            );
        }
        Overridden::Absent
    }

    /// The value a read finds, or `None` to carry on down the chain the read was walking.
    ///
    /// An accessor answers `None` here and is called by [`Vm::read_property`], which is the only
    /// place that can run one: this path is `&self` and a getter is ordinary code.
    pub(super) fn read(&self, target: &Value, key: &str) -> Option<Value> {
        match self.overridden(target, key) {
            Overridden::Held(Property::Value(value), _) => Some(value),
            Overridden::Held(Property::Accessor { .. }, _) | Overridden::Absent => {
                Some(Value::Undefined)
            }
            Overridden::Untouched => None,
        }
    }

    /// Whether any program has defined an accessor on any built-in.
    ///
    /// See the field: the answer guards a prototype-chain walk that would otherwise run on every
    /// read that misses a value's own store.
    pub(super) fn any_accessor(&self) -> bool {
        self.accessors.get()
    }

    /// Whether the default array-index lookup chain has been given an ordinary prototype link.
    /// A dense-array index search can skip holes only when both built-in links still match the
    /// chain it models: Array.prototype -> Object.prototype -> null. Direct numeric definitions
    /// have their own monotonic flag; this catches a custom prototype whose indexed properties
    /// live outside either intrinsic store.
    pub(super) fn indexed_prototype_chain_is_custom(&self) -> bool {
        self.indexed_prototype_chain_custom.get()
    }

    /// Whether this built-in still accepts new properties.
    ///
    /// An unmaterialized intrinsic is extensible by default; a materialized store carries the
    /// ordinary object extensibility bit.
    pub(super) fn extensible(&self, target: &Value) -> bool {
        let Some(identity) = intrinsic_key(target) else {
            return true;
        };
        self.entries
            .borrow()
            .iter()
            .find(|(held, _)| *held == identity)
            .is_none_or(|(_, data)| data.is_extensible())
    }

    /// Close a built-in against new properties, sealing what is already there if asked.
    ///
    /// The flag lives on the realm-owned ordinary store: the shared tables are immutable, and one
    /// realm closing `Math` must not close it for another.
    pub(super) fn close(&self, target: &Value, sealing: bool) {
        let Some(identity) = intrinsic_key(target) else {
            return;
        };
        self.materialize(target);
        let mut entries = self.entries.borrow_mut();
        let data = Self::data(&mut entries, identity);
        if sealing {
            data.seal();
        } else {
            data.prevent_extensions();
        }
    }

    pub(super) fn install_key(
        &self,
        target: &Value,
        key: PropertyKeyRef<'_>,
        property: Property,
        attributes: Attributes,
    ) {
        let Some(identity) = intrinsic_key(target) else {
            return;
        };
        if matches!(property, Property::Accessor { .. }) {
            self.accessors.set(true);
        }
        if matches!(
            target,
            Value::Native(
                Native::PrototypeOf(PrototypeTable::Array | PrototypeTable::Object),
                _
            )
        ) && array_index_key(key).is_some()
        {
            self.indexed_array_prototype.set(true);
        }
        if matches!(target, Value::Native(Native::NodeBufferGlobal, _))
            && key.matches_ascii("prototype")
            && let Property::Value(prototype) = &property
        {
            BUFFER_PROTOTYPE.with(|slot| *slot.borrow_mut() = Some(prototype.clone()));
        }
        self.materialize(target);
        let mut entries = self.entries.borrow_mut();
        let data = Self::data(&mut entries, identity);
        data.install_key(key, property, attributes);
    }

    pub(super) fn remove_key(&self, target: &Value, key: PropertyKeyRef<'_>) {
        let Some(identity) = intrinsic_key(target) else {
            return;
        };
        self.materialize(target);
        let mut entries = self.entries.borrow_mut();
        let data = Self::data(&mut entries, identity);
        data.remove_key(key);
    }

    /// Complete materialized own keys in ordinary ObjectData insertion order.
    pub(super) fn is_materialized(&self, target: &Value) -> bool {
        intrinsic_key(target).is_some_and(|identity| {
            self.entries
                .borrow()
                .iter()
                .any(|(key, _)| *key == identity)
        })
    }

    pub(super) fn own_keys_units(&self, target: &Value) -> Option<Vec<StringUnits>> {
        let identity = intrinsic_key(target)?;
        let entries = self.entries.borrow();
        let data = &entries.iter().find(|(key, _)| *key == identity)?.1;
        Some(data.keys().cloned().collect())
    }

    pub(super) fn enumerable_entries_units(&self, target: &Value) -> Vec<(StringUnits, Property)> {
        let Some(identity) = intrinsic_key(target) else {
            return Vec::new();
        };
        self.materialize(target);
        let entries = self.entries.borrow();
        let Some((_, data)) = entries.iter().find(|(held, _)| *held == identity) else {
            return Vec::new();
        };
        data.string_entries()
            .map(|(key, property)| (key.clone(), property.clone()))
            .collect()
    }

    pub(super) fn data(
        entries: &mut Vec<(IntrinsicKey, ObjectData)>,
        identity: IntrinsicKey,
    ) -> &mut ObjectData {
        let position = entries.iter().position(|(key, _)| *key == identity);
        let position = position.unwrap_or_else(|| {
            entries.push((identity, ObjectData::default()));
            entries.len() - 1
        });
        &mut entries[position].1
    }

    pub(super) fn materialize(&self, target: &Value) {
        let Some(identity) = intrinsic_key(target) else {
            return;
        };
        if self
            .entries
            .borrow()
            .iter()
            .any(|(key, _)| *key == identity)
        {
            return;
        }
        let realm = value_realm(target);
        if let Value::Native(
            Native::PrototypeOf(
                table @ (PrototypeTable::Symbol
                | PrototypeTable::Array
                | PrototypeTable::Number
                | PrototypeTable::Boolean
                | PrototypeTable::BigInt
                | PrototypeTable::Promise
                | PrototypeTable::Object
                | PrototypeTable::Function
                | PrototypeTable::Map
                | PrototypeTable::Set
                | PrototypeTable::WeakMap
                | PrototypeTable::WeakSet),
            ),
            _,
        ) = target
        {
            let proto_id = match table {
                PrototypeTable::Symbol => kiln_builtin_surface::PrototypeId::Symbol,
                PrototypeTable::Array => kiln_builtin_surface::PrototypeId::Array,
                PrototypeTable::Number => kiln_builtin_surface::PrototypeId::Number,
                PrototypeTable::Boolean => kiln_builtin_surface::PrototypeId::Boolean,
                PrototypeTable::BigInt => kiln_builtin_surface::PrototypeId::BigInt,
                PrototypeTable::Promise => kiln_builtin_surface::PrototypeId::Promise,
                PrototypeTable::Object => kiln_builtin_surface::PrototypeId::Object,
                PrototypeTable::Function => kiln_builtin_surface::PrototypeId::Function,
                PrototypeTable::Map => kiln_builtin_surface::PrototypeId::Map,
                PrototypeTable::Set => kiln_builtin_surface::PrototypeId::Set,
                PrototypeTable::WeakMap => kiln_builtin_surface::PrototypeId::WeakMap,
                PrototypeTable::WeakSet => kiln_builtin_surface::PrototypeId::WeakSet,
                _ => unreachable!(),
            };
            let mut data = ObjectData::default();
            for member in kiln_builtin_surface::installed_prototype_members(proto_id) {
                let key_str = match member.key {
                    kiln_builtin_surface::MemberKey::Named(name) => name,
                    kiln_builtin_surface::MemberKey::Symbol(sym) => sym.property_key(),
                };
                let prop = match member.binding {
                    kiln_builtin_surface::SemanticBinding::PrimitiveToString => {
                        Property::Value(Value::Native(Native::PrimitiveToString, realm))
                    }
                    kiln_builtin_surface::SemanticBinding::PrimitiveValueOf => {
                        Property::Value(Value::Native(Native::SymbolValueOf, realm))
                    }
                    kiln_builtin_surface::SemanticBinding::SymbolDescription => {
                        Property::Accessor {
                            get: Some(Value::Native(
                                Native::BuiltinAccessor(BuiltinAccessor::SymbolDescription),
                                realm,
                            )),
                            set: None,
                        }
                    }
                    kiln_builtin_surface::SemanticBinding::StringTag(tag) => {
                        Property::Value(Value::String(tag.to_owned().into()))
                    }
                    kiln_builtin_surface::SemanticBinding::SymbolToPrimitive => {
                        Property::Value(Value::Native(Native::SymbolToPrimitive, realm))
                    }
                    kiln_builtin_surface::SemanticBinding::Constructor => {
                        let ctor = match table {
                            PrototypeTable::Symbol => Native::SymbolConstructor,
                            PrototypeTable::Array => Native::ArrayConstructor,
                            PrototypeTable::Number => Native::NumberConstructor,
                            PrototypeTable::Boolean => Native::BooleanConstructor,
                            PrototypeTable::BigInt => Native::BigIntConstructor,
                            PrototypeTable::Promise => Native::PromiseConstructor,
                            PrototypeTable::Object => Native::ObjectConstructor,
                            PrototypeTable::Function => Native::FunctionConstructor,
                            PrototypeTable::Map => Native::MapConstructor,
                            PrototypeTable::Set => Native::SetConstructor,
                            PrototypeTable::WeakMap => Native::WeakMapConstructor,
                            PrototypeTable::WeakSet => Native::WeakSetConstructor,
                            _ => unreachable!(),
                        };
                        Property::Value(Value::Native(ctor, realm))
                    }
                    kiln_builtin_surface::SemanticBinding::ArrayLength => {
                        Property::Value(Value::Number(0.0))
                    }
                    kiln_builtin_surface::SemanticBinding::ArrayUnscopables => {
                        Property::Value(self.array_unscopables(realm))
                    }
                    kiln_builtin_surface::SemanticBinding::ArrayMethod(method) => {
                        Property::Value(array_method_value(method, realm))
                    }
                    kiln_builtin_surface::SemanticBinding::NumberMethod(method) => {
                        Property::Value(number_method_value(method, realm))
                    }
                    kiln_builtin_surface::SemanticBinding::BooleanMethod(
                        kiln_builtin_surface::BooleanMethod::ToString,
                    ) => Property::Value(Value::Native(Native::BooleanToString, realm)),
                    kiln_builtin_surface::SemanticBinding::BooleanMethod(
                        kiln_builtin_surface::BooleanMethod::ValueOf,
                    ) => Property::Value(Value::Native(Native::BooleanValueOf, realm)),
                    kiln_builtin_surface::SemanticBinding::BigIntMethod(
                        kiln_builtin_surface::BigIntMethod::ToString
                        | kiln_builtin_surface::BigIntMethod::ToLocaleString,
                    ) => Property::Value(Value::Native(Native::BigIntToString, realm)),
                    kiln_builtin_surface::SemanticBinding::BigIntMethod(
                        kiln_builtin_surface::BigIntMethod::ValueOf,
                    ) => Property::Value(Value::Native(Native::BigIntValueOf, realm)),
                    kiln_builtin_surface::SemanticBinding::PromiseMethod(
                        kiln_builtin_surface::PromiseMethod::Then,
                    ) => Property::Value(Value::Native(Native::PromiseThen, realm)),
                    kiln_builtin_surface::SemanticBinding::PromiseMethod(
                        kiln_builtin_surface::PromiseMethod::Catch,
                    ) => Property::Value(Value::Native(Native::PromiseCatch, realm)),
                    kiln_builtin_surface::SemanticBinding::PromiseMethod(
                        kiln_builtin_surface::PromiseMethod::Finally,
                    ) => Property::Value(Value::Native(Native::PromiseFinally, realm)),
                    kiln_builtin_surface::SemanticBinding::ObjectMethod(method) => {
                        Property::Value(object_method_value(method, realm))
                    }
                    kiln_builtin_surface::SemanticBinding::ObjectProto => Property::Accessor {
                        get: Some(Value::Native(Native::ProtoGetter, realm)),
                        set: Some(Value::Native(Native::ProtoSetter, realm)),
                    },
                    kiln_builtin_surface::SemanticBinding::FunctionMethod(method) => {
                        Property::Value(function_method_value(method, realm))
                    }
                    kiln_builtin_surface::SemanticBinding::FunctionHasInstance => {
                        Property::Value(Value::Native(Native::FunctionHasInstance, realm))
                    }
                    kiln_builtin_surface::SemanticBinding::FunctionCaller
                    | kiln_builtin_surface::SemanticBinding::FunctionArguments => {
                        Property::Accessor {
                            get: Some(Value::Native(Native::ThrowTypeError, realm)),
                            set: Some(Value::Native(Native::ThrowTypeError, realm)),
                        }
                    }
                    kiln_builtin_surface::SemanticBinding::MapMethod(method) => {
                        Property::Value(map_method_value(method, realm))
                    }
                    kiln_builtin_surface::SemanticBinding::SetMethod(method) => {
                        Property::Value(set_method_value(method, realm))
                    }
                    kiln_builtin_surface::SemanticBinding::WeakMapMethod(method) => {
                        Property::Value(weak_map_method_value(method, realm))
                    }
                    kiln_builtin_surface::SemanticBinding::WeakSetMethod(method) => {
                        Property::Value(weak_set_method_value(method, realm))
                    }
                    kiln_builtin_surface::SemanticBinding::CollectionSize(is_map) => {
                        Property::Accessor {
                            get: Some(Value::Native(
                                Native::BuiltinAccessor(BuiltinAccessor::CollectionSize(is_map)),
                                realm,
                            )),
                            set: None,
                        }
                    }
                    kiln_builtin_surface::SemanticBinding::CollectionIterator(is_map) => {
                        Property::Value(Value::Native(
                            if is_map {
                                Native::CollectionEntries(true)
                            } else {
                                Native::CollectionValues(false)
                            },
                            realm,
                        ))
                    }
                    // The shared bootstrap defines it (ADR 0060 kind 7). The slot is reserved
                    // here, in the surface's order, so that its definition lands where node
                    // lists it rather than after everything this table installed.
                    kiln_builtin_surface::SemanticBinding::Bootstrap => {
                        Property::Value(Value::Undefined)
                    }
                };
                let attrs = Attributes {
                    writable: member.attributes.writable,
                    enumerable: member.attributes.enumerable,
                    configurable: member.attributes.configurable,
                };
                data.install_key(PropertyKeyRef::Utf8(key_str), prop, attrs);
            }
            data.set_prototype(prototype_of(target));
            let mut entries = self.entries.borrow_mut();
            if let Some((_, slot)) = entries.iter_mut().find(|(key, _)| *key == identity) {
                *slot = data;
            } else {
                entries.push((identity, data));
            }
            return;
        }
        let mut keys: Vec<String> = match target {
            Value::Native(Native::PrototypeOf(table), _) => intrinsic_prototype_entries(*table)
                .into_iter()
                .map(|(key, _)| key)
                .collect(),
            _ => Vec::new(),
        };
        if !matches!(target, Value::Native(Native::PrototypeOf(_), _)) {
            if intrinsic_identity(target).is_some() {
                keys.extend(["length".into(), "name".into()]);
            }
            if intrinsic_prototype_in(self, target).is_some() {
                keys.push("prototype".into());
            }
            keys.extend(intrinsic_extra_property_names(target));
            keys.extend(
                intrinsic_static_property_names(target)
                    .iter()
                    .map(|key| (*key).to_owned()),
            );
        }
        for symbol in WellKnown::ALL {
            let key = if matches!(target, Value::Native(Native::SymbolConstructor, _)) {
                symbol.js_name()
            } else {
                symbol.property_key()
            };
            if !keys.iter().any(|existing| existing == key) {
                keys.push(key.to_owned());
            }
        }
        let mut data = ObjectData::default();
        for key in keys {
            if let Some(attributes) = tabulated_attributes(self, target, &key)
                && let Some(property) = tabulated_property(self, target, &key)
            {
                data.install_key(
                    PropertyKeyRef::Utf8(&key),
                    retag_intrinsic_property(property, realm),
                    attributes,
                );
            } else if bootstrap_static(target, &key) {
                // The shared bootstrap defines it (ADR 0060 kind 7). The slot is reserved in the
                // surface's order, so that the definition lands where node lists the member --
                // `Array.fromAsync` before `Array.of` -- rather than after everything else.
                data.install_key(
                    PropertyKeyRef::Utf8(&key),
                    Property::Value(Value::Undefined),
                    Attributes {
                        writable: true,
                        enumerable: false,
                        configurable: true,
                    },
                );
            }
        }
        data.set_prototype(prototype_of(target));
        let mut entries = self.entries.borrow_mut();
        if let Some((_, slot)) = entries.iter_mut().find(|(key, _)| *key == identity) {
            *slot = data;
        } else {
            entries.push((identity, data));
        }
    }
}

/// Whether `key` is a static of `target` that the shared bootstrap owns
/// (`kiln_builtin_surface::BOOTSTRAP_STATICS`).
fn bootstrap_static(target: &Value, key: &str) -> bool {
    let global = match target {
        Value::Native(Native::ObjectConstructor, _) => "Object",
        Value::Native(Native::ArrayConstructor, _) => "Array",
        Value::Native(Native::PromiseConstructor, _) => "Promise",
        Value::Native(Native::Reflect, _) => "Reflect",
        Value::Native(Native::RegExpConstructor, _) => "RegExp",
        Value::Native(Native::IteratorConstructor, _) => "Iterator",
        _ => return false,
    };
    kiln_builtin_surface::is_bootstrap_static(global, key)
}

/// What a built-in inherits, for a read that has got past its own properties.
///
/// A namespace and a method table are ordinary objects and reach `Object.prototype`; everything
/// else here is callable and reaches `Function.prototype` first. The distinction matters only
/// after a deletion -- a program that removed `Function.prototype.call` must not find it again
/// one link up -- which is why a table goes straight to the object methods.
pub(super) fn native_inherited_property(
    intrinsics: &Intrinsics,
    target: &Value,
    name: &str,
) -> Value {
    let realm = value_realm(target);
    match target {
        // NativeError constructors inherit static properties from Error. Keep this separate from
        // the own-property table: `TypeError.isError` is inherited, so it must not appear in
        // TypeError's own property names or descriptor.
        Value::Native(Native::Error(kind), _) if *kind != ErrorKind::Error => {
            match intrinsics.overridden(
                &Value::Native(Native::Error(ErrorKind::Error), realm),
                name,
            ) {
                Overridden::Held(Property::Value(value), _) => value,
                Overridden::Held(Property::Accessor { .. }, _) | Overridden::Absent => {
                    function_property(intrinsics, name, realm)
                }
                Overridden::Untouched => Native::Error(ErrorKind::Error)
                    .property(name)
                    .unwrap_or_else(|| function_property(intrinsics, name, realm)),
            }
        }
        Value::Native(Native::PrototypeOf(PrototypeTable::TypedArrayFor(_)), _) => match table_step(
            intrinsics,
            PrototypeTable::TypedArray,
            name,
            Native::PrototypeOf(PrototypeTable::TypedArray)
                .property(name)
                .unwrap_or(Value::Undefined),
        realm) {
            Value::Undefined => inherited_object_property(intrinsics, name, realm),
            found => found,
        },
        // The five iterator prototypes and `%GeneratorPrototype%` sit one link below
        // `%IteratorPrototype%`, and it is one link below `Object.prototype`.
        Value::Native(
            Native::PrototypeOf(PrototypeTable::IteratorFor(_) | PrototypeTable::Generator),
            _,
        ) => match iterator_prototype_property(intrinsics, name, realm) {
            Value::Undefined => inherited_object_property(intrinsics, name, realm),
            found => found,
        },
        Value::Native(Native::PrototypeOf(_) | Native::Math | Native::Json | Native::Reflect, _)
        // The `Intl` namespace and the six prototypes are ordinary objects, so they inherit
        // from `Object.prototype` rather than from `Function.prototype`. An `Intl` instance
        // does too: its own methods are found first, and everything else comes from there.
        | Value::Native(Native::Intl(intl::Op::Namespace | intl::Op::Prototype(_)), _)
        | Value::Intl(_) => inherited_object_property(intrinsics, name, realm),
        _ => function_property(intrinsics, name, realm),
    }
}

/// One step of an implicit prototype chain, with the realm's store consulted first.
///
/// A value whose chain reaches a built-in's method table has no object to walk into: the table
/// is a `match`, and the step is a call to it. That is where a property the program put on
/// `Object.prototype` or `Array.prototype` has to be found, or the write would land somewhere no
/// read could reach. `Undefined` continues down the chain, which is what makes a *deleted* table
/// entry fall through to the next table rather than shadowing it with nothing.
pub(super) fn table_step(
    intrinsics: &Intrinsics,
    table: PrototypeTable,
    name: &str,
    tabulated: Value,
    realm: RealmId,
) -> Value {
    intrinsics
        .read(&Value::Native(Native::PrototypeOf(table), realm), name)
        .unwrap_or_else(|| {
            if matches!(table, PrototypeTable::Array)
                && name == WellKnown::Unscopables.property_key()
            {
                intrinsics.array_unscopables(realm)
            } else {
                in_realm(tabulated, realm)
            }
        })
}

/// A read that has reached `%IteratorPrototype%`: the realm's store first, then the table.
///
/// Every iterator this realm makes -- an array, string, collection or regexp walk, a generator,
/// a helper -- inherits from it, and the store is where the shared bootstrap
/// (`kiln-builtins/src/iterator.ts`) puts `chunks`, `windows`, `join` and `@@dispose`, and where
/// a program's own `Iterator.prototype.x = ...` lands. The per-kind tables step up to the
/// `%IteratorPrototype%` *table* themselves, which has no store to consult, so a name they do not
/// answer is carried here by the reads that can.
pub(super) fn iterator_prototype_property(
    intrinsics: &Intrinsics,
    name: &str,
    realm: RealmId,
) -> Value {
    table_step(
        intrinsics,
        PrototypeTable::Iterator,
        name,
        iterator_method(name),
        realm,
    )
}

/// The names physically owned by each virtual intrinsic prototype table.
///
/// Reads can fall through to `Object.prototype`, but reflection must not report inherited names
/// as own properties. Keeping this inventory beside the table machinery gives the virtual
/// prototypes the same observable shape as ordinary objects.
pub(super) const UINT8_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&[
        "BYTES_PER_ELEMENT",
        "toBase64",
        "toHex",
        "setFromBase64",
        "setFromHex",
    ]);
/// Every concrete view but `Uint8Array` publishes only its element width; the byte view adds the
/// four text conversions, which is why it has a list of its own.
pub(super) const TYPED_ARRAY_VIEW_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&["BYTES_PER_ELEMENT"]);
pub(super) const ARRAY_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&[
        "map",
        "forEach",
        "filter",
        "reduce",
        "sort",
        "findIndex",
        "find",
        "some",
        "every",
        "flatMap",
        "findLast",
        "findLastIndex",
        "reduceRight",
        "join",
        "push",
        "pop",
        "indexOf",
        "includes",
        "slice",
        "at",
        "flat",
        "entries",
        "keys",
        "concat",
        "reverse",
        "shift",
        "unshift",
        "lastIndexOf",
        "fill",
        "splice",
        "copyWithin",
        "toString",
        "toLocaleString",
        "values",
        "with",
        "toReversed",
        "toSorted",
        "toSpliced",
    ]);
pub(super) const STRING_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&[
        "values",
        "toUpperCase",
        "toLowerCase",
        "includes",
        "indexOf",
        "slice",
        "substring",
        "substr",
        "split",
        "trim",
        "startsWith",
        "endsWith",
        "repeat",
        "at",
        "codePointAt",
        "localeCompare",
        "toLocaleUpperCase",
        "toLocaleLowerCase",
        "normalize",
        "charAt",
        "charCodeAt",
        "padStart",
        "padEnd",
        "replace",
        "replaceAll",
        "lastIndexOf",
        "trimStart",
        "trimEnd",
        "concat",
        "toString",
        "valueOf",
        "match",
        "matchAll",
        "search",
        "trimLeft",
        "trimRight",
        "anchor",
        "big",
        "blink",
        "bold",
        "fixed",
        "fontcolor",
        "fontsize",
        "italics",
        "link",
        "small",
        "strike",
        "sub",
        "sup",
    ]);

pub(super) const REG_EXP_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&[
        "exec",
        "dotAll",
        "flags",
        "global",
        "hasIndices",
        "ignoreCase",
        "multiline",
        "source",
        "sticky",
        "unicode",
        "unicodeSets",
        "toString",
        "test",
    ]);
pub(super) const ARRAY_BUFFER_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&["slice", "sliceToImmutable", "transferToImmutable"]);
pub(super) const DATA_VIEW_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&[
        "getInt8",
        "getUint8",
        "getInt16",
        "getUint16",
        "getInt32",
        "getUint32",
        "getFloat32",
        "getFloat64",
        "setInt8",
        "setUint8",
        "setInt16",
        "setUint16",
        "setInt32",
        "setUint32",
        "setFloat32",
        "setFloat64",
        "getBigInt64",
        "getBigUint64",
        "setBigInt64",
        "setBigUint64",
    ]);
/// `%GeneratorPrototype%` and `%AsyncGeneratorPrototype%` publish the same three names. The
/// `constructor` the specification gives them is an intrinsic this engine has no object for, so
/// it is absent rather than wrong -- see `Native::property`.
/// Each of the five iterator prototypes owns exactly `next`; the tag is symbol-keyed and listed
/// through the symbol path.
pub(super) const BUILT_IN_ITERATOR_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&["next"]);

pub(super) const GENERATOR_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&["next", "return", "throw"]);

pub(super) const ITERATOR_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&[
        "map", "filter", "take", "drop", "flatMap", "reduce", "toArray", "forEach", "some",
        "every", "find", "includes",
    ]);
pub(super) const WEAK_REF_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&["deref"]);
pub(super) const SHADOW_REALM_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&["evaluate", "importValue"]);
pub(super) const DISPOSABLE_STACK_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&["use", "adopt", "defer", "move", "dispose"]);
pub(super) const ASYNC_DISPOSABLE_STACK_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&["use", "adopt", "defer", "move", "disposeAsync"]);
pub(super) const FINALIZATION_REGISTRY_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&["register", "unregister"]);
// Every name `events::prototype_property` answers: a name that reads but is missing here is
// invisible to `in`, `hasOwnProperty` and `getOwnPropertyNames` while still callable. The order
// is the native backend's (`kiln_c99_install_event_emitter_methods`), which enumeration exposes.
pub(super) const EVENT_EMITTER_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&[
        "on",
        "addListener",
        "once",
        "emit",
        "listeners",
        "listenerCount",
        "removeListener",
        "off",
        "removeAllListeners",
        "setMaxListeners",
        "eventNames",
    ]);
pub(super) const WRITABLE_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&[
        "write",
        "end",
        "destroy",
        "on",
        "addListener",
        "once",
        "emit",
        "listeners",
        "listenerCount",
        "removeListener",
        "off",
        "removeAllListeners",
        "setMaxListeners",
        "eventNames",
    ]);
pub(super) const CONSOLE_MEMBERS: intrinsic_members::NamedMembers =
    intrinsic_members::NamedMembers::new(&[
        "assert",
        "clear",
        "count",
        "countReset",
        "debug",
        "dir",
        "dirxml",
        "error",
        "group",
        "groupCollapsed",
        "groupEnd",
        "info",
        "log",
        "profile",
        "profileEnd",
        "table",
        "time",
        "timeEnd",
        "timeLog",
        "timeStamp",
        "trace",
        "warn",
    ]);

/// The string-keyed members each built-in prototype lists.
///
/// Each list is a `const` built through [`intrinsic_members::NamedMembers`], which refuses a
/// symbol's internal `@@` spelling *while the compiler is building it*. That is not decoration:
/// putting `"@@hasInstance"` in one of these lists once leaked the internal spelling of
/// `Symbol.hasInstance` into every property listing in the realm, and nothing about the change
/// looked wrong. Now it does not build.
pub(super) fn intrinsic_prototype_method_names(table: PrototypeTable) -> &'static [&'static str] {
    match table {
        // A concrete view's prototype carries only what differs between the views: its element
        // width, and -- for the byte view alone -- the four conversions to and from text.
        PrototypeTable::TypedArrayFor(kind) => match kind {
            TypedKind::Uint8 => UINT8_MEMBERS.names(),
            _ => TYPED_ARRAY_VIEW_MEMBERS.names(),
        },
        PrototypeTable::Array => ARRAY_MEMBERS.names(),
        PrototypeTable::String => STRING_MEMBERS.names(),
        PrototypeTable::TypedArray
        | PrototypeTable::Function
        | PrototypeTable::Object
        | PrototypeTable::Number
        | PrototypeTable::Boolean
        | PrototypeTable::BigInt
        | PrototypeTable::Promise
        | PrototypeTable::Symbol
        | PrototypeTable::Map
        | PrototypeTable::Set
        | PrototypeTable::WeakMap
        | PrototypeTable::WeakSet
        | PrototypeTable::SharedArrayBuffer
        // `getTime` and `setTime` are still answered by the table, for the shared
        // `Date.prototype` to capture, and listed by the realm's store once it has redefined
        // them -- in the order it defines every member, which is node's.
        | PrototypeTable::Date => &[],
        PrototypeTable::RegExp => REG_EXP_MEMBERS.names(),
        PrototypeTable::ArrayBuffer => ARRAY_BUFFER_MEMBERS.names(),
        PrototypeTable::DataView => DATA_VIEW_MEMBERS.names(),
        PrototypeTable::Iterator => ITERATOR_MEMBERS.names(),
        PrototypeTable::Generator | PrototypeTable::AsyncGenerator => GENERATOR_MEMBERS.names(),
        PrototypeTable::IteratorFor(_) => BUILT_IN_ITERATOR_MEMBERS.names(),
        PrototypeTable::WeakRef => WEAK_REF_MEMBERS.names(),
        PrototypeTable::ShadowRealm => SHADOW_REALM_MEMBERS.names(),
        PrototypeTable::DisposableStack => DISPOSABLE_STACK_MEMBERS.names(),
        PrototypeTable::AsyncDisposableStack => ASYNC_DISPOSABLE_STACK_MEMBERS.names(),
        PrototypeTable::FinalizationRegistry => FINALIZATION_REGISTRY_MEMBERS.names(),
        PrototypeTable::EventEmitter => EVENT_EMITTER_MEMBERS.names(),
        PrototypeTable::Readable => stream::readable_prototype_member_names(),
        PrototypeTable::Writable => WRITABLE_MEMBERS.names(),
        PrototypeTable::Console => CONSOLE_MEMBERS.names(),
        PrototypeTable::AsyncIterator | PrototypeTable::WebStub(_) => &[],
    }
}

pub(super) fn intrinsic_prototype_entries(table: PrototypeTable) -> Vec<(String, Property)> {
    if let Some(proto_id) = match table {
        PrototypeTable::Symbol => Some(kiln_builtin_surface::PrototypeId::Symbol),
        PrototypeTable::Array => Some(kiln_builtin_surface::PrototypeId::Array),
        PrototypeTable::Number => Some(kiln_builtin_surface::PrototypeId::Number),
        PrototypeTable::Boolean => Some(kiln_builtin_surface::PrototypeId::Boolean),
        PrototypeTable::BigInt => Some(kiln_builtin_surface::PrototypeId::BigInt),
        PrototypeTable::Promise => Some(kiln_builtin_surface::PrototypeId::Promise),
        PrototypeTable::Object => Some(kiln_builtin_surface::PrototypeId::Object),
        PrototypeTable::Function => Some(kiln_builtin_surface::PrototypeId::Function),
        PrototypeTable::Map => Some(kiln_builtin_surface::PrototypeId::Map),
        PrototypeTable::Set => Some(kiln_builtin_surface::PrototypeId::Set),
        PrototypeTable::WeakMap => Some(kiln_builtin_surface::PrototypeId::WeakMap),
        PrototypeTable::WeakSet => Some(kiln_builtin_surface::PrototypeId::WeakSet),
        _ => None,
    } {
        return kiln_builtin_surface::installed_prototype_members(proto_id)
            .filter_map(|member| {
                let name = match member.key {
                    kiln_builtin_surface::MemberKey::Named(name) => name,
                    kiln_builtin_surface::MemberKey::Symbol(_) => return None,
                };
                let prop = match member.binding {
                    kiln_builtin_surface::SemanticBinding::PrimitiveToString => {
                        Property::Value(Value::Native(Native::PrimitiveToString, RealmId::PRIMARY))
                    }
                    kiln_builtin_surface::SemanticBinding::PrimitiveValueOf => {
                        Property::Value(Value::Native(Native::SymbolValueOf, RealmId::PRIMARY))
                    }
                    kiln_builtin_surface::SemanticBinding::SymbolDescription => {
                        Property::Accessor {
                            get: Some(Value::Native(
                                Native::BuiltinAccessor(BuiltinAccessor::SymbolDescription),
                                RealmId::PRIMARY,
                            )),
                            set: None,
                        }
                    }
                    kiln_builtin_surface::SemanticBinding::Constructor => {
                        let ctor = match table {
                            PrototypeTable::Symbol => Native::SymbolConstructor,
                            PrototypeTable::Array => Native::ArrayConstructor,
                            PrototypeTable::Number => Native::NumberConstructor,
                            PrototypeTable::Boolean => Native::BooleanConstructor,
                            PrototypeTable::BigInt => Native::BigIntConstructor,
                            PrototypeTable::Promise => Native::PromiseConstructor,
                            PrototypeTable::Object => Native::ObjectConstructor,
                            PrototypeTable::Function => Native::FunctionConstructor,
                            PrototypeTable::Map => Native::MapConstructor,
                            PrototypeTable::Set => Native::SetConstructor,
                            PrototypeTable::WeakMap => Native::WeakMapConstructor,
                            PrototypeTable::WeakSet => Native::WeakSetConstructor,
                            _ => unreachable!(),
                        };
                        Property::Value(Value::Native(ctor, RealmId::PRIMARY))
                    }
                    kiln_builtin_surface::SemanticBinding::ArrayLength => {
                        Property::Value(Value::Number(0.0))
                    }
                    kiln_builtin_surface::SemanticBinding::ArrayMethod(method) => {
                        Property::Value(array_method_value(method, RealmId::PRIMARY))
                    }
                    kiln_builtin_surface::SemanticBinding::NumberMethod(method) => {
                        Property::Value(number_method_value(method, RealmId::PRIMARY))
                    }
                    kiln_builtin_surface::SemanticBinding::BooleanMethod(
                        kiln_builtin_surface::BooleanMethod::ToString,
                    ) => Property::Value(Value::Native(Native::BooleanToString, RealmId::PRIMARY)),
                    kiln_builtin_surface::SemanticBinding::BooleanMethod(
                        kiln_builtin_surface::BooleanMethod::ValueOf,
                    ) => Property::Value(Value::Native(Native::BooleanValueOf, RealmId::PRIMARY)),
                    kiln_builtin_surface::SemanticBinding::BigIntMethod(
                        kiln_builtin_surface::BigIntMethod::ToString
                        | kiln_builtin_surface::BigIntMethod::ToLocaleString,
                    ) => Property::Value(Value::Native(Native::BigIntToString, RealmId::PRIMARY)),
                    kiln_builtin_surface::SemanticBinding::BigIntMethod(
                        kiln_builtin_surface::BigIntMethod::ValueOf,
                    ) => Property::Value(Value::Native(Native::BigIntValueOf, RealmId::PRIMARY)),
                    kiln_builtin_surface::SemanticBinding::PromiseMethod(
                        kiln_builtin_surface::PromiseMethod::Then,
                    ) => Property::Value(Value::Native(Native::PromiseThen, RealmId::PRIMARY)),
                    kiln_builtin_surface::SemanticBinding::PromiseMethod(
                        kiln_builtin_surface::PromiseMethod::Catch,
                    ) => Property::Value(Value::Native(Native::PromiseCatch, RealmId::PRIMARY)),
                    kiln_builtin_surface::SemanticBinding::PromiseMethod(
                        kiln_builtin_surface::PromiseMethod::Finally,
                    ) => Property::Value(Value::Native(Native::PromiseFinally, RealmId::PRIMARY)),
                    kiln_builtin_surface::SemanticBinding::ObjectMethod(method) => {
                        Property::Value(object_method_value(method, RealmId::PRIMARY))
                    }
                    kiln_builtin_surface::SemanticBinding::ObjectProto => Property::Accessor {
                        get: Some(Value::Native(Native::ProtoGetter, RealmId::PRIMARY)),
                        set: Some(Value::Native(Native::ProtoSetter, RealmId::PRIMARY)),
                    },
                    kiln_builtin_surface::SemanticBinding::FunctionMethod(method) => {
                        Property::Value(function_method_value(method, RealmId::PRIMARY))
                    }
                    kiln_builtin_surface::SemanticBinding::FunctionCaller
                    | kiln_builtin_surface::SemanticBinding::FunctionArguments => {
                        Property::Accessor {
                            get: Some(Value::Native(Native::ThrowTypeError, RealmId::PRIMARY)),
                            set: Some(Value::Native(Native::ThrowTypeError, RealmId::PRIMARY)),
                        }
                    }
                    kiln_builtin_surface::SemanticBinding::MapMethod(method) => {
                        Property::Value(map_method_value(method, RealmId::PRIMARY))
                    }
                    kiln_builtin_surface::SemanticBinding::SetMethod(method) => {
                        Property::Value(set_method_value(method, RealmId::PRIMARY))
                    }
                    kiln_builtin_surface::SemanticBinding::WeakMapMethod(method) => {
                        Property::Value(weak_map_method_value(method, RealmId::PRIMARY))
                    }
                    kiln_builtin_surface::SemanticBinding::WeakSetMethod(method) => {
                        Property::Value(weak_set_method_value(method, RealmId::PRIMARY))
                    }
                    kiln_builtin_surface::SemanticBinding::CollectionSize(is_map) => {
                        Property::Accessor {
                            get: Some(Value::Native(
                                Native::BuiltinAccessor(BuiltinAccessor::CollectionSize(is_map)),
                                RealmId::PRIMARY,
                            )),
                            set: None,
                        }
                    }
                    _ => return None,
                };
                Some((name.to_owned(), prop))
            })
            .collect();
    }
    let target = Value::Native(Native::PrototypeOf(table), RealmId::PRIMARY);
    let mut names = vec!["constructor"];
    if matches!(table, PrototypeTable::String) {
        names.push("length");
    }
    names
        .into_iter()
        .chain(intrinsic_prototype_method_names(table).iter().copied())
        .chain(intrinsic_members::accessor_keys(table))
        .filter_map(|name| {
            if let Some(value) = string_exotic(&target, name) {
                return Some((name.to_owned(), Property::Value(value)));
            }
            if let Some(accessor) = intrinsic_accessor(&target, name) {
                return Some((name.to_owned(), accessor));
            }
            match Native::PrototypeOf(table).property(name) {
                Some(Value::Undefined) | None => None,
                Some(value) => Some((name.to_owned(), Property::Value(value))),
            }
        })
        .collect()
}

/// The getter a built-in publishes under one name, or `None` for a name that is not an accessor.
///
/// One place, so that reading the property, describing it and listing it cannot disagree about
/// which of the two shapes it is -- a disagreement `verifyProperty` sees immediately.
pub(super) fn intrinsic_accessor(target: &Value, key: &str) -> Option<Property> {
    let realm = value_realm(target);
    match target {
        Value::Native(Native::PrototypeOf(PrototypeTable::RegExp), _) => {
            RegExpAccessor::named(key).map(|accessor| Property::Accessor {
                get: Some(Value::Native(Native::RegExpAccessor(accessor), realm)),
                // No setter half at all, which is what makes `re.global = true` silently do
                // nothing rather than store a shadowing property.
                set: None,
            })
        }
        // Everything a built-in prototype publishes about its instances: `byteLength`, `size`,
        // and their eleven siblings. One table, so that reading the property, describing it and
        // listing it cannot disagree.
        Value::Native(Native::PrototypeOf(table), _)
            if prototype_accessor(*table, key).is_some() =>
        {
            prototype_accessor(*table, key).map(|accessor| Property::Accessor {
                get: Some(Value::Native(Native::BuiltinAccessor(accessor), realm)),
                set: None,
            })
        }
        // Every field one of the eight `Temporal` types publishes -- `blank`, `dayOfWeek`,
        // `epochNanoseconds` and their thirty-seven siblings. The language spells all of them as
        // getters on the prototype rather than as data properties on the instance, and the
        // difference is what a program sees: `Temporal.Duration.prototype.blank` is not a flag no
        // duration has, it is a getter whose brand check refuses the prototype.
        Value::Native(Native::Temporal(temporal::Op::Prototype(kind)), _) => {
            temporal::prototype_accessor(*kind, key).map(|field| Property::Accessor {
                get: Some(Value::Native(
                    Native::Temporal(temporal::Op::Accessor(*kind, field)),
                    realm,
                )),
                // No setter half: `duration.blank = true` writes nothing, exactly as
                // `re.global = true` does.
                set: None,
            })
        }
        // `Symbol.species` is one accessor defined eight times over, always the same: it
        // answers its own receiver.
        Value::Native(native, _)
            if defines_species(*native) && key == WellKnown::Species.property_key() =>
        {
            Some(Property::Accessor {
                get: Some(Value::Native(
                    Native::BuiltinAccessor(BuiltinAccessor::Species),
                    realm,
                )),
                set: None,
            })
        }
        // The restricted pair. Both halves are the *same* function, and a program can compare
        // them, so both spell it the same way.
        Value::Native(Native::PrototypeOf(PrototypeTable::Function), _)
            if matches!(key, "caller" | "arguments") =>
        {
            Some(Property::Accessor {
                get: Some(Value::Native(Native::ThrowTypeError, realm)),
                set: Some(Value::Native(Native::ThrowTypeError, realm)),
            })
        }
        Value::Native(Native::PrototypeOf(PrototypeTable::Object), _) if key == "__proto__" => {
            Some(Property::Accessor {
                get: Some(Value::Native(Native::ProtoGetter, realm)),
                set: Some(Value::Native(Native::ProtoSetter, realm)),
            })
        }
        // `%TypedArray%.prototype[Symbol.toStringTag]` is an accessor: it reports the concrete
        // view name from its receiver and returns undefined for an incompatible receiver. Real
        // serializers capture this getter once and call it as their typed-array brand check.
        Value::Native(Native::PrototypeOf(PrototypeTable::TypedArray), _)
            if key == WellKnown::ToStringTag.property_key() =>
        {
            Some(Property::Accessor {
                get: Some(Value::Native(Native::TypedArrayToStringTag, realm)),
                set: None,
            })
        }
        _ => None,
    }
}

/// The own, program-visible keys of a value, in insertion order.
pub(super) fn own_property_names_units(
    intrinsics: &Intrinsics,
    target: &Value,
) -> Vec<StringUnits> {
    let from_store = |data: &ObjectData| data.keys().cloned().collect::<Vec<_>>();
    match target {
        Value::Object(object) => {
            let mut keys = match wrapped_primitive(target) {
                Some(Value::String(text)) => (0..text.unit_len())
                    .map(|index| StringUnits::from(index.to_string()))
                    .collect(),
                _ => Vec::new(),
            };
            keys.extend(from_store(&object.borrow()));
            if matches!(wrapped_primitive(target), Some(Value::String(_))) {
                keys.push(StringUnits::from("length"));
            }
            keys
        }
        Value::Function(closure) => {
            let mut keys = from_store(&closure.properties.borrow());
            if has_prototype_property(closure) {
                keys.retain(|key| *key != "prototype");
                let statics = keys.split_off(2.min(keys.len()));
                keys.push(StringUnits::from("prototype"));
                keys.extend(statics);
            }
            keys
        }
        Value::Bound(bound) => [StringUnits::from("length"), StringUnits::from("name")]
            .into_iter()
            .chain(from_store(&bound.properties.borrow()))
            .collect(),
        Value::Array(items) => {
            let mut keys = items
                .borrow()
                .iter()
                .enumerate()
                .filter(|(_, item)| !matches!(item, Value::Native(Native::Hole, _)))
                .map(|(index, _)| StringUnits::from(index.to_string()))
                .collect::<Vec<_>>();
            keys.push(StringUnits::from("length"));
            keys.extend(from_store(&items.properties.borrow()));
            keys
        }
        Value::TypedArray(view) => (0..view.len())
            .map(|index| StringUnits::from(index.to_string()))
            .chain(from_store(&view.properties.borrow()))
            .collect(),
        other if exotic_properties(other).is_some() => exotic_properties(target)
            .map(|store| from_store(&store.borrow()))
            .unwrap_or_default(),
        _ => own_property_keys_units(intrinsics, target),
    }
}

/// `[[OwnPropertyKeys]]` as display strings, for the tests that list a built-in.
#[cfg(test)]
pub(super) fn own_property_keys(intrinsics: &Intrinsics, target: &Value) -> Vec<String> {
    own_property_keys_units(intrinsics, target)
        .into_iter()
        .map(|key| key.to_lossy_string())
        .collect()
}

/// `[[OwnPropertyKeys]]` in the exact UTF-16 key representation.
///
/// The scalar wrapper above exists only for display-oriented helpers. VM reflection, proxy
/// invariants, and descriptor walks use this function so a lone surrogate remains the same
/// property from the store through the trap and back into the operation.
pub(super) fn own_property_keys_units(intrinsics: &Intrinsics, target: &Value) -> Vec<StringUnits> {
    if matches!(target, Value::Native(_, _) | Value::Callback(_, _)) {
        intrinsics.materialize(target);
    }
    let from_object = |data: &ObjectData| -> Vec<StringUnits> {
        data.properties
            .iter()
            .filter(|slot| !slot.symbol && !slot.internal)
            .map(|slot| slot.key.clone())
            .collect()
    };
    match target {
        Value::Shared(facade) => facade.own_keys(),
        // A String wrapper lists its characters first, in index order, and then whatever the
        // program put on the box -- the order the specification's `OrdinaryOwnPropertyKeys`
        // gives, where integer indices always precede the rest.
        Value::Object(object) if matches!(wrapped_primitive(target), Some(Value::String(_))) => {
            let Some(Value::String(text)) = wrapped_primitive(target) else {
                unreachable!("checked by the guard");
            };
            let mut keys: Vec<StringUnits> = (0..text.unit_len())
                .map(|index| StringUnits::from(index.to_string()))
                .collect();
            keys.extend(from_object(&object.borrow()));
            keys.push(StringUnits::from("length"));
            keys
        }
        Value::Bound(bound) => {
            let mut keys = vec![StringUnits::from("length"), StringUnits::from("name")];
            keys.extend(from_object(&bound.properties.borrow()));
            keys
        }
        Value::Object(object) => from_object(&object.borrow()),
        Value::Function(closure) => {
            let mut keys = from_object(&closure.properties.borrow());
            // After `length` and `name`, which the closure stores, and before whatever the
            // program hung on the function itself -- the order the specification lists them in.
            if has_prototype_property(closure) {
                keys.retain(|key| *key != "prototype");
                let statics = keys.split_off(2.min(keys.len()));
                keys.push(StringUnits::from("prototype"));
                keys.extend(statics);
            }
            keys
        }
        Value::Native(Native::PrototypeOf(_table), _) => {
            intrinsics.own_keys_units(target).unwrap_or_default()
        }
        Value::Native(_, _)
        | Value::Callback(_, _)
        | Value::Settler(..)
        | Value::Capability(..)
        | Value::Element(_) => {
            let mut keys = intrinsics.own_keys_units(target).unwrap_or_default();
            // Promise combinators and capability machinery create these runtime callables rather
            // than intrinsics, so they have no materialized store. Their identity slots are still
            // own properties, and CreateBuiltinFunction installs them in length/name order. Keep
            // that order visible to OrdinaryOwnPropertyKeys.
            if matches!(
                target,
                Value::Settler(..) | Value::Capability(..) | Value::Element(_)
            ) && intrinsic_identity(target).is_some()
            {
                keys.splice(
                    0..0,
                    [StringUnits::from("length"), StringUnits::from("name")],
                );
            }
            keys
        }
        Value::Array(items) => {
            // An accessor leaves a hole in the dense storage, so the indices that are really
            // there are the union of the two: the walk below would otherwise list an array's
            // getters as absent.
            let mut indices: Vec<usize> = items
                .borrow()
                .iter()
                .enumerate()
                .filter(|(_, item)| !matches!(item, Value::Native(Native::Hole, _)))
                .map(|(index, _)| index)
                .collect();
            let defined: Vec<usize> = items
                .elements
                .borrow()
                .iter()
                .map(|entry| entry.index)
                .collect();
            for index in defined {
                if !indices.contains(&index) {
                    indices.push(index);
                }
            }
            indices.sort_unstable();
            let mut keys: Vec<StringUnits> = indices
                .iter()
                .map(|index| StringUnits::from(index.to_string()))
                .collect();
            // After the indices and before whatever the program hung on the array: `length` is
            // an own property created with the array, and the specification lists integer
            // indices first and everything else in creation order.
            keys.push(StringUnits::from("length"));
            keys.extend(from_object(&items.properties.borrow()));
            keys
        }
        // The elements first, in index order, and then whatever the program hung on the view --
        // the order `OrdinaryOwnPropertyKeys` gives, where integer indices always precede the
        // rest.
        Value::TypedArray(view) => {
            let mut keys: Vec<StringUnits> = (0..view.len())
                .map(|index| StringUnits::from(index.to_string()))
                .collect();
            keys.extend(from_object(&view.properties.borrow()));
            keys
        }
        other if exotic_properties(other).is_some() => exotic_properties(target)
            .map(|store| from_object(&store.borrow()))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Names supplied by a native constructor/namespace's compact property table.
///
/// Native values do not carry an `ObjectData`, so `own_property_keys` cannot discover these by
/// walking a store. Keep this candidate list next to the own-key walk and filter it through
/// `own_attributes`: a name is included only when the same property machinery can describe it.
pub(super) fn intrinsic_extra_property_names(target: &Value) -> Vec<String> {
    match target {
        Value::Native(Native::Temporal(op), _) => match op {
            temporal::Op::Prototype(kind) => temporal::property_names(*op)
                .iter()
                .chain(temporal::prototype_accessor_names(*kind).iter())
                .map(|name| (*name).to_owned())
                .collect(),
            _ => temporal::property_names(*op)
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
        },
        Value::Native(Native::Intl(op), _) => intl::property_names(*op)
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
        Value::Native(native, _) => native
            .own_property_rows()
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn intrinsic_static_property_names(target: &Value) -> &'static [&'static str] {
    match target {
        // `from` and `of` live on `%TypedArray%` and are inherited by all concrete constructors.
        // `Uint8Array` alone receives the two TypeScript-installed byte codecs as own statics.
        Value::Native(Native::TypedArrayConstructor(TypedKind::Uint8), _) => {
            return &["BYTES_PER_ELEMENT", "fromBase64", "fromHex"];
        }
        Value::Native(Native::TypedArrayConstructor(_), _) => {
            return &["BYTES_PER_ELEMENT"];
        }
        Value::Native(Native::TypedArrayIntrinsic, _) => return &["of", "from"],
        _ => {}
    }
    let tabulated = intrinsic_tabulated_property_names(target);
    if !tabulated.is_empty() {
        return tabulated;
    }
    // A Node builtin namespace publishes what the shared ABI table says it publishes, which is
    // the same list the ESM projection binds. Without it the namespace had *no* own keys at all:
    // `Object.keys(require("path"))` was 0 against node's 17, so every `{ ...require("path") }`
    // re-export shim a bundler emits produced an empty object while every individual member
    // still read correctly -- a silent hole rather than a failure.
    //
    // Consulted only where the tables above say nothing, because `node:buffer` is *both* the
    // module and the `Buffer` constructor here, and the constructor's statics are the answer
    // that arm was written to give.
    if let Value::Native(native, _) = target
        && let Some(specifier) = node_builtin_specifier(*native)
        && let Some(exports) = kiln_builtins::builtin_exports_for_specifier(specifier)
    {
        return exports;
    }
    &[]
}

/// The static members a built-in carries, out of the table both engines read.
///
/// ONE TABLE, TWO EMITTERS (ADR 0043). The member names used to be written out here and again as
/// `kiln_c99_same_name` chains in `kiln-backend-c`'s emitted C, and nothing compared the two. They
/// drifted, silently, until an npm bundle read a member one of them lacked: `Number` had fifteen
/// statics here and five there, and `Number.MAX_VALUE` is the first property a bundled `zod`
/// reads, so its absence refused the whole module.
///
/// Every arm below binds a `const`, so [`kiln_builtin_surface::members`] runs at compile time and
/// a global this interpreter names but the shared table does not declare fails to build. What the
/// arm still chooses is which built-in it *is* -- that is this interpreter's own knowledge of its
/// `Native` values, and the shared table has no business holding it.
pub(super) fn intrinsic_tabulated_property_names(target: &Value) -> &'static [&'static str] {
    match target {
        Value::Native(Native::ArrayConstructor, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("Array");
            MEMBERS
        }
        Value::Native(Native::ObjectConstructor, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("Object");
            MEMBERS
        }
        Value::Native(Native::StringConstructor, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("String");
            MEMBERS
        }
        Value::Native(Native::NumberConstructor, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("Number");
            MEMBERS
        }
        Value::Native(Native::DateConstructor, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("Date");
            MEMBERS
        }
        Value::Native(Native::PromiseConstructor, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("Promise");
            MEMBERS
        }
        Value::Native(Native::Json, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("JSON");
            MEMBERS
        }
        Value::Native(Native::Reflect, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("Reflect");
            MEMBERS
        }
        Value::Native(Native::ProxyConstructor, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("Proxy");
            MEMBERS
        }
        Value::Native(Native::ArrayBufferConstructor, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("ArrayBuffer");
            MEMBERS
        }
        Value::Native(Native::RegExpConstructor, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("RegExp");
            MEMBERS
        }
        Value::Native(Native::Error(ErrorKind::Error), _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("Error");
            MEMBERS
        }
        Value::Native(Native::BigIntConstructor, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("BigInt");
            MEMBERS
        }
        Value::Native(Native::MapConstructor, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("Map");
            MEMBERS
        }
        Value::Native(Native::SymbolConstructor, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("Symbol");
            MEMBERS
        }
        Value::Native(Native::IteratorConstructor, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("Iterator");
            MEMBERS
        }
        // The *global* `Buffer` alone. `node:buffer` is both this module and that constructor
        // here, and answering the constructor's list for it published eight names where the
        // shared ABI declares ten -- so the two engines disagreed about what
        // `Object.keys(require("node:buffer"))` is, and `Buffer` and `default` were names the
        // native projection published and this one did not. The namespace falls through to the
        // Node ABI arm below, which is the list both engines bind.
        Value::Native(Native::NodeBufferGlobal, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("Buffer");
            MEMBERS
        }
        Value::Native(Native::Atomics, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("Atomics");
            MEMBERS
        }
        Value::Native(Native::Math, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("Math");
            MEMBERS
        }
        Value::Native(Native::Performance, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("performance");
            MEMBERS
        }
        Value::Native(Native::TypedArrayIntrinsic, _) => {
            const MEMBERS: &[&str] = kiln_builtin_surface::members("%TypedArray%");
            MEMBERS
        }
        Value::Native(Native::TypedArrayConstructor(kind), _) => match kind {
            // The complete own-property list above is intentionally narrower than the surface
            // table: from/of remain readable through `%TypedArray%`, but aren't own here.
            _ => &[],
        },
        _ => &[],
    }
}

/// The own-key walk used by operations whose specification includes symbol keys.
///
/// Most reflection helpers in this interpreter intentionally return only names because the
/// current compact symbol representation cannot reconstruct a `Symbol` value from its reserved
/// slot. Internal algorithms do not need that reconstruction: preserving the slot is enough to
/// read and define the same symbol-keyed property without losing its identity.
/// Whether a reserved store key names a symbol-keyed property rather than an internal slot.
///
/// The two share the `@@` prefix, which is what made reflection unable to tell them apart and so
/// report neither. A key is a symbol's exactly when a well-known symbol claims it or it carries
/// a `Symbol()` serial; everything else -- `@@entries`, `@@trace`, `@@disposeEntries`,
/// `@@private:x` -- is the machine's own bookkeeping and is not a property of the object at all.
pub(super) fn is_symbol_key(key: &str) -> bool {
    WellKnown::from_property_key(key).is_some() || Symbol::serial_in_key(key).is_some()
}

/// The exact-key counterpart to [`symbol::is_reserved`]. Internal slots and symbol keys are
/// ASCII by construction, so a malformed UTF-16 key cannot become one through replacement.
pub(super) fn is_reserved_units(key: &StringUnits) -> bool {
    if key.unit_len() < 2
        || key.unit_at(0) != Some(b'@' as u16)
        || key.unit_at(1) != Some(b'@' as u16)
    {
        return false;
    }
    PropertyKeyRef::Units(key)
        .known_ascii()
        .is_some_and(|name| symbol::is_reserved(name.as_ref()))
}

/// `OrdinaryOwnPropertyKeys`: the string keys, then the symbol keys, each in insertion order.
///
/// Distinct from [`own_property_keys_including_symbols`], which hands back every reserved slot
/// and is what an internal algorithm wants; this is the list a *program* is allowed to see.
#[cfg(test)]
pub(super) fn own_property_keys_with_symbols(
    intrinsics: &Intrinsics,
    target: &Value,
) -> Vec<String> {
    own_property_keys_with_symbols_units(intrinsics, target)
        .into_iter()
        .map(|key| key.to_lossy_string())
        .collect()
}

#[cfg(test)]
pub(super) fn own_property_keys_including_symbols(
    intrinsics: &Intrinsics,
    target: &Value,
) -> Vec<String> {
    own_property_keys_including_symbols_units(intrinsics, target)
        .into_iter()
        .map(|key| key.to_lossy_string())
        .collect()
}

pub(super) fn own_property_keys_with_symbols_units(
    intrinsics: &Intrinsics,
    target: &Value,
) -> Vec<StringUnits> {
    own_property_keys_including_symbols_units(intrinsics, target)
}

pub(super) fn own_property_keys_including_symbols_units(
    intrinsics: &Intrinsics,
    target: &Value,
) -> Vec<StringUnits> {
    let mut keys = own_property_keys_units(intrinsics, target);
    keys.extend(own_symbol_keys_units(intrinsics, target));
    keys
}

/// Symbol slots retain their key domain even when a string slot has identical UTF-16 units.
pub(super) fn own_symbol_keys_units(intrinsics: &Intrinsics, target: &Value) -> Vec<StringUnits> {
    // Ordinary reflection intentionally hides symbol slots, but internal operations such as
    // proxy invariants and `Object.defineProperties` consume the full own-key list. Append the
    // reserved slots after the ordinary names, preserving each store's insertion order.
    let mut keys: Vec<StringUnits> = match target {
        Value::Object(object) => object
            .borrow()
            .properties
            .iter()
            .filter(|slot| slot.symbol)
            .map(|slot| slot.key.clone())
            .collect(),
        Value::Function(closure) => closure
            .properties
            .borrow()
            .properties
            .iter()
            .filter(|slot| slot.symbol)
            .map(|slot| slot.key.clone())
            .collect(),
        Value::Bound(bound) => bound
            .properties
            .borrow()
            .properties
            .iter()
            .filter(|slot| slot.symbol)
            .map(|slot| slot.key.clone())
            .collect(),
        Value::Array(items) => items
            .properties
            .borrow()
            .properties
            .iter()
            .filter(|slot| slot.symbol)
            .map(|slot| slot.key.clone())
            .collect(),

        other if exotic_properties(other).is_some() => exotic_properties(target)
            .map(|store| {
                store
                    .borrow()
                    .properties
                    .iter()
                    .filter(|slot| slot.symbol)
                    .map(|slot| slot.key.clone())
                    .collect()
            })
            .unwrap_or_default(),
        Value::Native(_, _)
        | Value::Callback(_, _)
        | Value::Settler(..)
        | Value::Capability(..)
        | Value::Element(_) => intrinsic_key(target)
            .and_then(|identity| {
                intrinsics.entries.borrow().iter().find_map(|(held, data)| {
                    (*held == identity).then(|| {
                        data.properties
                            .iter()
                            .filter(|slot| slot.symbol)
                            .map(|slot| slot.key.clone())
                            .collect()
                    })
                })
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    // A built-in's symbol-keyed members. They are answered by the value tables and listed by
    // nothing, so `Object.getOwnPropertySymbols(Date.prototype)` was `[]` where every other
    // engine reports `Symbol.toPrimitive` -- the property read correctly, described correctly,
    // and did not exist as far as reflection was concerned.
    //
    // Derived by asking the table for each well-known symbol rather than from a list of which
    // ones each built-in has: such a list is a second place to describe a member, and the whole
    // point here is that there is one. `WellKnown::ALL` is itself compiler-checked, so a new
    // symbol cannot be missed. This is deliberately *not* in `own_property_keys`, which
    // `Object.getOwnPropertyNames` reads unfiltered -- putting a reserved spelling there is
    // exactly the leak that cost 838 rows.
    if matches!(target, Value::Native(_, _)) && !intrinsics.is_materialized(target) {
        let Value::Native(native, _) = target else {
            unreachable!("checked by the guard");
        };
        for symbol in WellKnown::ALL {
            let key = symbol.property_key();
            // An accessor as well as a value: `%TypedArray%.prototype[@@toStringTag]` is a
            // getter, so asking the value tables alone left it readable, describable, and
            // absent from `getOwnPropertySymbols`.
            let published = !matches!(native.property(key), None | Some(Value::Undefined))
                || intrinsic_accessor(target, key).is_some()
                || machine_own_accessor(target, key).is_some()
                || (matches!(native, Native::PrototypeOf(PrototypeTable::Array))
                    && key == WellKnown::Unscopables.property_key());
            if published {
                keys.push(StringUnits::from(key));
            }
        }
    }
    // A built-in table and a materialized slot may describe one symbol. Deduplicate within
    // the Symbol domain only; a string of the same spelling remains a separate property.
    let mut stable = Vec::with_capacity(keys.len());
    let mut seen = HashSet::with_capacity(keys.len());
    for key in keys {
        if seen.insert(key.clone()) {
            stable.push(key);
        }
    }
    stable
}

/// The three flags an own property carries, or `None` when there is no such own property.
///
/// The one shape without a slot of its own is an array element: it lives in the dense storage,
/// which has room for a value and nothing else, so the only thing that can lock one down is
/// freezing the array it belongs to.
pub(super) fn own_attributes(
    intrinsics: &Intrinsics,
    target: &Value,
    key: &str,
) -> Option<Attributes> {
    own_attributes_key(intrinsics, target, PropertyKeyRef::Utf8(key))
}

pub(super) fn own_attributes_key(
    intrinsics: &Intrinsics,
    target: &Value,
    key: PropertyKeyRef<'_>,
) -> Option<Attributes> {
    match target {
        Value::Shared(facade) => facade.own_property(key).map(|(_, attributes)| attributes),
        Value::Object(object) => object.borrow().get_own_attributes_key(key).or_else(|| {
            // A String wrapper's own characters, which no store holds: they are derived from the
            // boxed string and are locked, exactly as a string primitive's are.
            string_exotic_key(target, key).is_some().then(|| {
                if key.matches_ascii("length") {
                    Attributes::NONE
                } else {
                    Attributes {
                        writable: false,
                        enumerable: true,
                        configurable: false,
                    }
                }
            })
        }),
        Value::Function(closure) => closure
            .properties
            .borrow()
            .get_own_attributes_key(key)
            // `prototype` is the one own property a function keeps outside its property store:
            // it lives in a slot of its own so that `new` and a read of `f.prototype` cannot
            // disagree about which object instances link to. It is still an own property, and a
            // locked one -- a program may replace the object, never remove the slot.
            .or_else(|| {
                (key.matches_ascii("prototype") && has_prototype_property(closure)).then_some(
                    Attributes {
                        writable: !closure.program.functions[closure.function as usize].is_class
                            && !closure.properties.borrow().is_frozen(),
                        enumerable: false,
                        configurable: false,
                    },
                )
            }),
        Value::Bound(bound) => bound
            .properties
            .borrow()
            .get_own_attributes_key(key)
            .or_else(|| tabulated_attributes_key(intrinsics, target, key)),
        Value::Array(items) if key.matches_ascii("length") => Some(Attributes {
            writable: items.length_writable.get(),
            enumerable: false,
            configurable: false,
        }),
        Value::Array(items) => match array_index_key(key) {
            Some(index) => {
                let frozen = items.properties.borrow().is_frozen();
                // A definition the program made wins over the ordinary flags, and freezing wins
                // over both: a frozen array's elements are locked whatever they were defined as.
                if let Some(entry) = array_exotic_element(items, index) {
                    return Some(if frozen {
                        Attributes {
                            writable: false,
                            enumerable: entry.attributes.enumerable,
                            configurable: false,
                        }
                    } else {
                        entry.attributes
                    });
                }
                let present = matches!(
                    items.borrow().get(index),
                    Some(item) if !matches!(item, Value::Native(Native::Hole, _))
                );
                if !present {
                    return None;
                }
                Some(if frozen {
                    Attributes {
                        writable: false,
                        enumerable: true,
                        configurable: false,
                    }
                } else {
                    Attributes::DEFAULT
                })
            }
            None => items.properties.borrow().get_own_attributes_key(key),
        },
        // An integer-indexed property's flags are fixed by the specification rather than
        // stored: every live element is writable, enumerable and configurable, and every other
        // element key is simply absent.
        Value::TypedArray(view) => match canonical_numeric_index_key(key) {
            Some(index) => valid_integer_index(view, index).map(|_| Attributes::DEFAULT),
            None => view.properties.borrow().get_own_attributes_key(key),
        },
        other if exotic_properties(other).is_some() => exotic_properties(target)
            .filter(|_| !exotic_reserved_name_key(target, key))
            .and_then(|store| store.borrow().get_own_attributes_key(key)),
        // A string's characters are own properties, and immovable ones: nothing can rewrite or
        // delete `"abc"[1]`, which is why they are the one shape here with no writable slot.
        Value::String(text) => string_index_attributes_key(text, key),
        // A built-in owns its `name` and `length` the same way a compiled function does, and a
        // namespace or a method table owns whatever the tables answer for. None of it is stored
        // per value -- two reads of `Array.prototype.push` produce the same `Copy` marker, so
        // there is no per-value store to put it in -- so the realm's store answers first, and
        // the tables answer for everything it has nothing to say about.
        Value::Native(_, _)
        | Value::Callback(_, _)
        | Value::Settler(..)
        | Value::Capability(..)
        | Value::Element(_) => {
            if matches!(target, Value::Native(_, _) | Value::Callback(_, _)) {
                intrinsics.materialize(target);
            }
            match intrinsics.overridden_key(target, key) {
                Overridden::Held(_, attributes) => Some(attributes),
                Overridden::Absent => None,
                Overridden::Untouched => tabulated_attributes_key(intrinsics, target, key),
            }
        }
        _ => None,
    }
}

/// The flags a built-in's own property was *created* with, before any program touched it.
pub(super) fn tabulated_attributes(
    intrinsics: &Intrinsics,
    target: &Value,
    key: &str,
) -> Option<Attributes> {
    tabulated_attributes_key(intrinsics, target, PropertyKeyRef::Utf8(key))
}

pub(super) fn tabulated_attributes_key(
    intrinsics: &Intrinsics,
    target: &Value,
    key: PropertyKeyRef<'_>,
) -> Option<Attributes> {
    let key = key.known_ascii()?;
    let key = key.as_ref();
    match key {
        "name" | "length" if intrinsic_identity(target).is_some() => Some(FUNCTION_IDENTITY),
        "length"
            if matches!(
                target,
                Value::Native(Native::PrototypeOf(PrototypeTable::Array), _)
            ) =>
        {
            Some(Attributes {
                writable: true,
                enumerable: false,
                configurable: false,
            })
        }
        // A built-in constructor's `prototype` is locked on every axis: the realm's intrinsics
        // are reachable through it, and letting a program move one would leave every instance
        // already made pointing at the old object.
        "prototype" if intrinsic_prototype_in(intrinsics, target).is_some() => {
            Some(Attributes::NONE)
        }
        _ => intrinsic_own_attributes(target, key),
    }
}

/// The flags on a built-in's own named property -- `Number.MAX_VALUE`, `Object.keys`,
/// `Array.prototype.push` -- or `None` when the built-in does not answer to that name.
///
/// The split is the specification's own convention and it falls out of what the property holds:
/// a method is replaceable and removable, while a *value* an intrinsic publishes -- a limit, a
/// well-known symbol, an element width -- is a constant, and locked on every axis.
/// The flags the own member `key` of a built-in carries, or `None` if there is no such member.
///
/// The existence test happens **once, here, before any rule can run**, and the rules themselves
/// live in [`published_attributes`], which returns `Attributes` rather than `Option<Attributes>`
/// and so cannot describe something that is not there.
///
/// That split is not tidiness. This function used to be a run of early returns, each deciding
/// flags from the *name* alone, and attributes are asked before the value is -- so a rule that
/// fired for a name the table does not publish did not merely mis-describe a property, it invented
/// one. Generalising a single rule from `Date` to every prototype gave every prototype in the
/// realm a phantom own `@@toPrimitive`; the fix at the time was to remember to add an existence
/// check to that rule and to the one beside it. Remembering is what failed in the first place.
pub(super) fn intrinsic_own_attributes(target: &Value, key: &str) -> Option<Attributes> {
    let Value::Native(native, _) = target else {
        return None;
    };
    if let Native::PrototypeOf(
        table @ (PrototypeTable::Symbol
        | PrototypeTable::Array
        | PrototypeTable::Number
        | PrototypeTable::Boolean
        | PrototypeTable::BigInt
        | PrototypeTable::Promise
        | PrototypeTable::Object
        | PrototypeTable::Function
        | PrototypeTable::Map
        | PrototypeTable::Set
        | PrototypeTable::WeakMap
        | PrototypeTable::WeakSet),
    ) = native
    {
        let proto_id = match table {
            PrototypeTable::Symbol => kiln_builtin_surface::PrototypeId::Symbol,
            PrototypeTable::Array => kiln_builtin_surface::PrototypeId::Array,
            PrototypeTable::Number => kiln_builtin_surface::PrototypeId::Number,
            PrototypeTable::Boolean => kiln_builtin_surface::PrototypeId::Boolean,
            PrototypeTable::BigInt => kiln_builtin_surface::PrototypeId::BigInt,
            PrototypeTable::Promise => kiln_builtin_surface::PrototypeId::Promise,
            PrototypeTable::Object => kiln_builtin_surface::PrototypeId::Object,
            PrototypeTable::Function => kiln_builtin_surface::PrototypeId::Function,
            PrototypeTable::Map => kiln_builtin_surface::PrototypeId::Map,
            PrototypeTable::Set => kiln_builtin_surface::PrototypeId::Set,
            PrototypeTable::WeakMap => kiln_builtin_surface::PrototypeId::WeakMap,
            PrototypeTable::WeakSet => kiln_builtin_surface::PrototypeId::WeakSet,
            _ => unreachable!(),
        };
        return kiln_builtin_surface::installed_prototype_member(proto_id, key).map(|member| {
            Attributes {
                writable: member.attributes.writable,
                enumerable: member.attributes.enumerable,
                configurable: member.attributes.configurable,
            }
        });
    }
    if string_exotic(target, key).is_some() {
        return Some(Attributes::NONE);
    }
    // Accessors are the one kind with no slot to look up -- the "value" is a getter -- so they
    // are settled before the value lookup rather than by it. Both of these answer only for names
    // that genuinely are accessors on this target, so neither can invent one.
    //
    // An accessor carries no `writable` at all: the flag is not part of its descriptor, so the
    // `false` is the absence of one rather than a claim about assignment.
    if intrinsic_accessor(target, key).is_some() || machine_own_accessor(target, key).is_some() {
        return Some(Attributes {
            writable: false,
            enumerable: false,
            configurable: true,
        });
    }
    // The single existence test, and it asks about *ownership* rather than about what a read
    // would find. `Native::property` carries a prototype table its last step up to
    // `Object.prototype`, which is right for a read and wrong here: asking it made every built-in
    // prototype in the realm claim `hasOwnProperty`, `valueOf` and the other nine as its own,
    // which `getOwnPropertyNames` denied in the same breath.
    let found = match intrinsic_owns(*native, key) {
        None | Some(Value::Undefined) => return None,
        Some(value) => value,
    };
    Some(published_attributes(target, *native, key, &found))
}

/// The flags a built-in member carries, given the member.
///
/// Total by design: every path answers, because the caller has already established that there is
/// something to answer about. There is no way to spell "and also this name does not exist" here,
/// which is the point -- that question has exactly one asker.
pub(super) fn published_attributes(
    target: &Value,
    native: Native,
    key: &str,
    found: &Value,
) -> Attributes {
    if matches!(
        native,
        Native::AsyncFunctionPrototype
            | Native::GeneratorFunctionPrototype
            | Native::AsyncGeneratorFunctionPrototype
    ) && key == "constructor"
    {
        return Attributes {
            writable: false,
            enumerable: false,
            configurable: true,
        };
    }
    // A Node builtin module's exports are *enumerable*, which is how `{ ...require("path") }`
    // and every `export * from "path"` re-export shim a bundler emits copies them.
    // `Object.keys(require("path"))` was 0 against node's 17 while every individual member read
    // correctly, so those shims produced empty objects and said nothing about it. This used to
    // name `node:buffer` alone, because that is the one module somebody had needed it for.
    //
    // The global `Buffer` is here for the same reason and was not: node builds its statics by
    // assignment, so `for (key in Buffer)` yields `isBuffer` and the rest, and `safer-buffer`
    // rebuilds its export by copying exactly what that loop finds. Ours were own, readable and
    // reported by `getOwnPropertyNames` -- but not enumerable, so the loop found nothing and
    // `safer.Buffer.isBuffer` came out undefined. `iconv-lite` takes its Buffer from there, and
    // express's body reader took the resulting TypeError, having already served a correct 200.
    if (node_builtin_specifier(native).is_some() || matches!(native, Native::NodeBufferGlobal))
        && intrinsic_static_property_names(target).contains(&key)
    {
        return Attributes::DEFAULT;
    }
    // An `@@toPrimitive` a prototype publishes is read-only wherever it appears -- `Date` and
    // `Symbol` both have one. It is what `ToPrimitive` finds before anything else, so a program
    // that could assign over it could change what every value of that type coerces to.
    if matches!(native, Native::PrototypeOf(_)) && key == WellKnown::ToPrimitive.property_key() {
        return Attributes {
            writable: false,
            enumerable: false,
            configurable: true,
        };
    }
    if matches!(native, Native::PrototypeOf(PrototypeTable::Array)) && key == "length" {
        return Attributes {
            writable: true,
            enumerable: false,
            configurable: false,
        };
    }
    // `Temporal.Now` is an object-valued built-in namespace property, not a constant. Node
    // exposes it as writable/configurable so clock shims (including Sinon fake timers) can
    // replace and later restore it. The generic non-function built-in rule below would lock it
    // down because `Temporal.Now` itself is not callable.
    if matches!(native, Native::Temporal(temporal::Op::Namespace)) && key == "Now" {
        return Attributes::BUILT_IN;
    }
    if found.type_of() == "function" {
        return Attributes::BUILT_IN;
    }
    // `Array.prototype[@@unscopables]` is a data property the specification locks against
    // assignment and leaves configurable, exactly as it does the tag below.
    if key == WellKnown::Unscopables.property_key() {
        return Attributes {
            writable: false,
            enumerable: false,
            configurable: true,
        };
    }
    // `@@toStringTag` is the one built-in *value* the specification leaves configurable, and it
    // has to be: the tag is what `Object.prototype.toString` reads, and a program that wants the
    // plain `[object Object]` back has no way to get it except by deleting the tag. Locked on
    // every axis, `delete Map.prototype[Symbol.toStringTag]` answered false and recorded nothing.
    // Below the callable test, as it has always been: the tag is a string, so the two cannot both
    // match, and keeping the order means this reshuffle changes nothing.
    if key == WellKnown::ToStringTag.property_key() {
        return Attributes {
            writable: false,
            enumerable: false,
            configurable: true,
        };
    }
    Attributes::NONE
}

/// The `prototype` a built-in constructor owns, or `None` for a built-in that is not one.
pub(super) fn intrinsic_prototype(target: &Value) -> Option<Value> {
    let Value::Native(native, _) = target else {
        return None;
    };
    // `Function.prototype` is itself callable, and it is the one intrinsic function that has no
    // `prototype` of its own -- asking it for one would otherwise loop straight back to itself.
    if matches!(native, Native::PrototypeOf(_)) {
        return None;
    }
    match native.property("prototype") {
        Some(Value::Undefined) | None => None,
        Some(prototype) => Some(prototype),
    }
}

/// [`intrinsic_prototype`], and the nine error constructors it cannot answer for.
///
/// Their prototypes are per-realm objects the store builds on demand rather than table entries,
/// so a `&Value` alone cannot reach one -- which is exactly why `Error` reported no own
/// `prototype` at all until this existed. Every caller here already holds the store.
pub(super) fn intrinsic_prototype_in(intrinsics: &Intrinsics, target: &Value) -> Option<Value> {
    if let Value::Native(Native::Error(kind), realm) = target {
        return Some(intrinsics.error_prototype(*kind, *realm));
    }
    intrinsic_prototype(target)
}

pub(super) fn string_index_attributes_key(
    text: &StringUnits,
    key: PropertyKeyRef<'_>,
) -> Option<Attributes> {
    if key.matches_ascii("length") {
        return Some(Attributes::NONE);
    }
    let index = array_index_key(key)?;
    (index < text.unit_len()).then_some(Attributes {
        writable: false,
        enumerable: true,
        configurable: false,
    })
}

/// One own property described as the object `Object.getOwnPropertyDescriptor` returns.
///
/// The shape and the flags both come from [`own_property_state`], which is what
/// `Object.defineProperty` validates against: a descriptor a program reads and a definition it
/// then makes have to be talking about the same property, and two walks of the same shapes would
/// be two chances to disagree.
pub(super) fn own_descriptor_key(
    intrinsics: &Intrinsics,
    target: &Value,
    key: PropertyKeyRef<'_>,
) -> Option<Value> {
    let (property, attributes) = own_property_state_key(intrinsics, target, key)?;
    let mut data = ObjectData::default();
    match property {
        Property::Value(value) => {
            data.set("value", value);
            data.set("writable", Value::Boolean(attributes.writable));
        }
        Property::Accessor { get, set } => {
            data.set("get", get.unwrap_or(Value::Undefined));
            data.set("set", set.unwrap_or(Value::Undefined));
        }
    }
    data.set("enumerable", Value::Boolean(attributes.enumerable));
    data.set("configurable", Value::Boolean(attributes.configurable));
    Some(Value::Object(tracked_object(data)))
}
