//! `node:diagnostics_channel` implementation.
//!
//! Provides diagnostic channel creation and tracing channel abstractions.

use crate::{Native, RealmId, Value, ordinary_object};

pub(crate) fn channel() -> Value {
    ordinary_object([
        ("hasSubscribers".to_owned(), Value::Boolean(false)),
        (
            "publish".to_owned(),
            Value::Native(Native::NodeDiagnosticsPublish, RealmId::PRIMARY),
        ),
        (
            "runStores".to_owned(),
            Value::Native(Native::NodeDiagnosticsRunStores, RealmId::PRIMARY),
        ),
    ])
}

pub(crate) fn tracing_channel() -> Value {
    let start = channel();
    let end = channel();
    let async_start = channel();
    let async_end = channel();
    let error = channel();
    ordinary_object([
        ("hasSubscribers".to_owned(), Value::Boolean(false)),
        ("start".to_owned(), start),
        ("end".to_owned(), end),
        ("asyncStart".to_owned(), async_start),
        ("asyncEnd".to_owned(), async_end),
        ("error".to_owned(), error),
    ])
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "channel" => Some(Value::Native(
            Native::NodeDiagnosticsChannelCreate,
            RealmId::PRIMARY,
        )),
        "tracingChannel" => Some(Value::Native(
            Native::NodeDiagnosticsTracingChannel,
            RealmId::PRIMARY,
        )),
        "hasSubscribers" => Some(Value::Boolean(false)),
        "default" => Some(Value::Native(
            Native::NodeDiagnosticsChannel,
            RealmId::PRIMARY,
        )),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeDiagnosticsChannel => Some("diagnostics_channel"),
        Native::NodeDiagnosticsChannelCreate => Some("channel"),
        Native::NodeDiagnosticsTracingChannel => Some("tracingChannel"),
        Native::NodeDiagnosticsPublish => Some("publish"),
        Native::NodeDiagnosticsRunStores => Some("runStores"),
        _ => None,
    }
}
