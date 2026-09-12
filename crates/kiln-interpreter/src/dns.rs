//! `node:dns` implementation.
//!
//! Provides deterministic DNS resolution primitives for loopback addresses.

use crate::{
    ArrayData, ExecutionError, Native, RealmId, Value, is_callable, ordinary_object,
    own_data_property,
};

/// Deterministic loopback-only resolution. Non-loopback DNS must become an
/// explicit async HostOp before it is advertised; it must never read ambient
/// resolver state from inside the interpreter.
pub(crate) fn prepare_lookup(
    arguments: &[Value],
    instruction: usize,
) -> Result<(Value, Vec<Value>), ExecutionError> {
    let host = arguments
        .first()
        .map(Value::to_display_string)
        .unwrap_or_else(|| "localhost".to_owned());
    let callback = arguments
        .iter()
        .rev()
        .find(|value| is_callable(value))
        .cloned()
        .unwrap_or(Value::Undefined);
    if !is_callable(&callback) {
        return Err(ExecutionError::NotCallable {
            instruction,
            value: callback.type_name(),
        });
    }

    let addresses = match host.as_str() {
        "localhost" => vec![
            ordinary_object([
                ("address".to_owned(), Value::String("127.0.0.1".to_owned())),
                ("family".to_owned(), Value::Number(4.0)),
            ]),
            ordinary_object([
                ("address".to_owned(), Value::String("::1".to_owned())),
                ("family".to_owned(), Value::Number(6.0)),
            ]),
        ],
        "127.0.0.1" => vec![ordinary_object([
            ("address".to_owned(), Value::String(host)),
            ("family".to_owned(), Value::Number(4.0)),
        ])],
        "::1" => vec![ordinary_object([
            ("address".to_owned(), Value::String(host)),
            ("family".to_owned(), Value::Number(6.0)),
        ])],
        _ => Vec::new(),
    };
    let options_all = arguments.get(1).is_some_and(|options| {
        own_data_property(options, "all").is_some_and(|value| value.is_truthy())
    });
    let callback_arguments = if addresses.is_empty() {
        vec![
            Value::String("ENOTFOUND".to_owned()),
            Value::Array(ArrayData::new(Vec::new())),
        ]
    } else if options_all {
        vec![Value::Null, Value::Array(ArrayData::new(addresses))]
    } else {
        let first = addresses.into_iter().next().unwrap();
        vec![
            Value::Null,
            own_data_property(&first, "address").unwrap_or(Value::Undefined),
            own_data_property(&first, "family").unwrap_or(Value::Undefined),
        ]
    };
    Ok((callback, callback_arguments))
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "lookup" => Some(Value::Native(Native::NodeDnsLookup, RealmId::PRIMARY)),
        "default" => Some(Value::Native(Native::NodeDns, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeDns => Some("dns"),
        Native::NodeDnsLookup => Some("lookup"),
        _ => None,
    }
}
