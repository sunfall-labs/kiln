//! `node:perf_hooks` implementation.
//!
//! Provides the Node performance hooks namespace.

use crate::{Native, RealmId, Value};

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "performance" => Some(Value::Native(Native::Performance, RealmId::PRIMARY)),
        "default" => Some(Value::Native(Native::NodePerfHooks, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodePerfHooks => Some("perf_hooks"),
        _ => None,
    }
}
