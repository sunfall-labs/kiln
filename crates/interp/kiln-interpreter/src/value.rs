use super::*;

pub(super) fn strict_equal(left: &Value, right: &Value) -> bool {
    // Exhaustive on `left` deliberately. A wildcard here made every new representation compile
    // as "unequal to everything", including itself. Adding a `Value` variant must keep failing to
    // compile until its JavaScript equality semantics are chosen explicitly.
    match left {
        Value::Undefined => matches!(right, Value::Undefined),
        Value::Null => matches!(right, Value::Null),
        Value::Boolean(a) => matches!(right, Value::Boolean(b) if a == b),
        Value::Number(a) => matches!(right, Value::Number(b) if a == b),
        Value::String(a) => matches!(right, Value::String(b) if a == b),
        // A cell is compiler bookkeeping and never observable as a JavaScript value. Identity is
        // still the least surprising answer if malformed bytecode lets one reach this boundary.
        Value::Cell(a) => matches!(right, Value::Cell(b) if Rc::ptr_eq(a, b)),
        // One instance of each runtime-provided callable exists per realm.
        // The realm is part of the identity, not decoration on it. `other.Array` and `Array`
        // name the same built-in and are two different functions, which is the single fact every
        // cross-realm test in the suite turns on.
        Value::Native(a, left_realm) => {
            matches!(right, Value::Native(b, right_realm) if a == b && left_realm == right_realm)
        }
        Value::Callback(a, left_realm) => {
            matches!(right, Value::Callback(b, right_realm) if a == b && left_realm == right_realm)
        }
        // Every object/function representation compares by identity: `===` asks whether two
        // references name the same object, not whether their payloads happen to be equal.
        Value::Generator(a) => matches!(right, Value::Generator(b) if Rc::ptr_eq(a, b)),
        Value::Promise(a) => matches!(right, Value::Promise(b) if Rc::ptr_eq(a, b)),
        Value::RegExp(a) => matches!(right, Value::RegExp(b) if Rc::ptr_eq(a, b)),
        Value::Date(a) => matches!(right, Value::Date(b) if Rc::ptr_eq(a, b)),
        Value::Temporal(a) => matches!(right, Value::Temporal(b) if Rc::ptr_eq(a, b)),
        Value::Intl(a) => matches!(right, Value::Intl(b) if Rc::ptr_eq(a, b)),
        // A BigInt is a primitive: two of them are equal when the numbers are, however they were
        // built. This is deliberately value equality rather than `Rc` identity.
        Value::BigInt(a) => matches!(right, Value::BigInt(b) if a == b),
        // Two capability executors are the same function only when they hold the same record:
        // each `NewPromiseCapability` makes its own, and a program can compare the one it was
        // handed with the one it kept.
        Value::Capability(a) => {
            matches!(right, Value::Capability(b) if Rc::ptr_eq(a, b))
        }
        Value::Settler(a, one) => matches!(
            right,
            Value::Settler(b, other) if Rc::ptr_eq(a, b) && one == other
        ),
        // Identity, like every other function: a program may hold one and compare it later.
        Value::Element(a) => matches!(right, Value::Element(b) if Rc::ptr_eq(a, b)),
        Value::Array(a) => matches!(right, Value::Array(b) if Rc::ptr_eq(a, b)),
        Value::Shared(a) => matches!(right, Value::Shared(b) if Rc::ptr_eq(a, b)),
        Value::TypedArray(a) => matches!(right, Value::TypedArray(b) if Rc::ptr_eq(a, b)),
        Value::ArrayBuffer(a) => matches!(right, Value::ArrayBuffer(b) if Rc::ptr_eq(a, b)),
        Value::DataView(a) => matches!(right, Value::DataView(b) if Rc::ptr_eq(a, b)),
        Value::Proxy(a) => matches!(right, Value::Proxy(b) if Rc::ptr_eq(a, b)),
        Value::Bound(a) => matches!(right, Value::Bound(b) if Rc::ptr_eq(a, b)),
        Value::Wrapped(a) => matches!(right, Value::Wrapped(b) if Rc::ptr_eq(a, b)),
        // `require` and `require.resolve` share lookup state but are distinct function objects.
        Value::CommonJsRequire(a) => {
            matches!(right, Value::CommonJsRequire(b) if Rc::ptr_eq(a, b))
        }
        Value::CommonJsResolve(a) => {
            matches!(right, Value::CommonJsResolve(b) if Rc::ptr_eq(a, b))
        }
        Value::Symbol(a) => matches!(right, Value::Symbol(b) if a == b),
        Value::Object(a) => matches!(right, Value::Object(b) if Rc::ptr_eq(a, b)),
        Value::Function(a) => matches!(right, Value::Function(b) if Rc::ptr_eq(a, b)),
    }
}

/// `==`, the abstract equality comparison.
///
/// Not `strict_equal` with a coercion in front of it. The rules are their own algorithm and two
/// of them are not conversions at all: `null == undefined` is true while neither converts to the
/// other, and every comparison with NaN is false however it is spelled.
pub(super) fn loose_equal_checked(left: &Value, right: &Value) -> Result<bool, ExecutionError> {
    Ok(match (left, right) {
        // Same type: `==` is `===`, including NaN being unequal to itself.
        (Value::Undefined | Value::Null, Value::Undefined | Value::Null) => true,
        (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_))
        | (Value::Boolean(_), Value::Boolean(_))
        | (Value::Symbol(_), Value::Symbol(_)) => strict_equal(left, right),
        // Nullish compares equal to itself and to nothing else -- not even to 0 or "".
        (Value::Undefined | Value::Null, _) | (_, Value::Undefined | Value::Null) => false,
        // A boolean converts to a number *first*, so `[] == false` is true by way of "" and 0.
        (Value::Boolean(value), other) => {
            loose_equal_checked(&Value::Number(f64::from(*value)), other)?
        }
        (other, Value::Boolean(value)) => {
            loose_equal_checked(other, &Value::Number(f64::from(*value)))?
        }
        // A symbol converts to nothing, so it is equal only to itself and that case is above.
        (Value::Symbol(_), _) | (_, Value::Symbol(_)) => false,
        // `==` is the one place the two numeric types meet: it compares mathematical values, so
        // `1n == 1` is true where `1n === 1` is false. A non-integral number can never equal a
        // BigInt, and neither can a string that is not an integer.
        (Value::BigInt(big), Value::Number(number))
        | (Value::Number(number), Value::BigInt(big)) => {
            number.is_finite()
                && number.fract() == 0.0
                && num_bigint::BigInt::parse_bytes(format!("{number:.0}").as_bytes(), 10)
                    .is_some_and(|other| other == **big)
        }
        (Value::BigInt(big), Value::String(text)) | (Value::String(text), Value::BigInt(big)) => {
            text.to_scalar_string()
                .ok()
                .as_deref()
                .unwrap_or("")
                .trim()
                .parse::<num_bigint::BigInt>()
                .is_ok_and(|other| other == **big)
        }
        (Value::Number(number), Value::String(text))
        | (Value::String(text), Value::Number(number)) => {
            Value::String(text.clone()).as_number() == *number
        }
        // An object compares by identity against another object, and by its primitive form
        // against anything else -- which is what makes `[1] == 1` true.
        (
            Value::Object(_) | Value::Array(_) | Value::Shared(_),
            Value::Object(_) | Value::Array(_) | Value::Shared(_),
        ) => strict_equal(left, right),
        (object @ (Value::Object(_) | Value::Array(_) | Value::Shared(_)), primitive) => {
            loose_equal_checked(&to_primitive(object)?, primitive)?
        }
        (primitive, object @ (Value::Object(_) | Value::Array(_) | Value::Shared(_))) => {
            loose_equal_checked(primitive, &to_primitive(object)?)?
        }
        _ => strict_equal(left, right),
    })
}
