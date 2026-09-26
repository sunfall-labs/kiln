//! `node:tty` implementation.
//!
//! Provides terminal detection primitives.

use crate::{Native, RealmId, Value};

/// Kiln captures the realm's standard streams. They are therefore not terminals,
/// regardless of whether the embedding process happens to have an ambient TTY.
pub(crate) fn isatty() -> Value {
    Value::Boolean(false)
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "isatty" => Some(Value::Native(Native::NodeTtyIsatty, RealmId::PRIMARY)),
        "default" => Some(Value::Native(Native::NodeTty, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeTty => Some("tty"),
        Native::NodeTtyIsatty => Some("isatty"),
        _ => None,
    }
}
