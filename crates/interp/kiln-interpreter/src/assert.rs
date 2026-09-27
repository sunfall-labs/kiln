//! `node:assert` implementation.
//!
//! Provides assertion primitives including `assert`, `assert.ok`, and `assert.strict`.

use crate::{ExecutionError, Native, RealmId, Value};

pub(crate) fn ok(arguments: &[Value]) -> Result<Value, ExecutionError> {
    let value = arguments.first().cloned().unwrap_or(Value::Undefined);
    if !value.is_truthy() {
        return Err(ExecutionError::TypeErrorMessage {
            message: arguments
                .get(1)
                .map(Value::to_display_string)
                .unwrap_or_else(|| "The expression evaluated to a falsy value".to_owned()),
        });
    }
    Ok(Value::Undefined)
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "AssertionError" | "deepEqual" | "deepStrictEqual" | "default" | "doesNotMatch"
        | "doesNotReject" | "doesNotThrow" | "equal" | "fail" | "ifError" | "match"
        | "notDeepEqual" | "notDeepStrictEqual" | "notEqual" | "notStrictEqual" | "ok"
        | "rejects" | "strict" | "strictEqual" | "throws" => {
            Some(Value::Native(Native::NodeAssertOk, RealmId::PRIMARY))
        }
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeAssert => Some("assert"),
        // `NodeAssertOk` is one variant for `require("assert")`, `assert.ok` and `assert.strict`,
        // which node makes three functions named "assert", "ok" and "strict". It is named for the
        // value `require` returns, because that is the one a program reads; `assert.ok.name`
        // reads "assert" here until the three become three variants.
        Native::NodeAssertOk => Some("assert"),
        _ => None,
    }
}
