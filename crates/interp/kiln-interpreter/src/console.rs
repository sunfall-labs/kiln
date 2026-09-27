//! `console` and `node:console` implementation.
//!
//! Provides the global `console` prototype methods, `Console` constructor statics,
//! and `node:console` module properties.

use crate::{Native, PrototypeTable, RealmId, Value, WellKnown};

/// Property lookup on `node:console`.
pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "default" => Some(Value::Native(Native::NodeConsole, RealmId::PRIMARY)),
        "Console" => Some(Value::Native(Native::ConsoleConstructor, RealmId::PRIMARY)),
        name => Native::PrototypeOf(PrototypeTable::Console).property(name),
    }
}

/// Property lookup on `Console` constructor.
pub(crate) fn constructor_property(prop: &str) -> Option<Value> {
    match prop {
        "prototype" => Some(Value::Native(
            Native::PrototypeOf(PrototypeTable::Console),
            RealmId::PRIMARY,
        )),
        _ => None,
    }
}

/// Property lookup on `Console.prototype`.
pub(crate) fn prototype_property(prop: &str) -> Option<Value> {
    match prop {
        "constructor" => Some(Value::Native(Native::ConsoleConstructor, RealmId::PRIMARY)),
        "log" => Some(Value::Native(Native::ConsoleLog, RealmId::PRIMARY)),
        "info" => Some(Value::Native(Native::ConsoleInfo, RealmId::PRIMARY)),
        "debug" => Some(Value::Native(Native::ConsoleDebug, RealmId::PRIMARY)),
        "dir" => Some(Value::Native(Native::ConsoleDir, RealmId::PRIMARY)),
        "warn" => Some(Value::Native(Native::ConsoleWarn, RealmId::PRIMARY)),
        "error" => Some(Value::Native(Native::ConsoleError, RealmId::PRIMARY)),
        "assert" => Some(Value::Native(Native::ConsoleAssert, RealmId::PRIMARY)),
        "clear" => Some(Value::Native(Native::ConsoleClear, RealmId::PRIMARY)),
        "count" => Some(Value::Native(Native::ConsoleCount, RealmId::PRIMARY)),
        "countReset" => Some(Value::Native(Native::ConsoleCountReset, RealmId::PRIMARY)),
        "group" => Some(Value::Native(Native::ConsoleGroup, RealmId::PRIMARY)),
        "groupEnd" => Some(Value::Native(Native::ConsoleGroupEnd, RealmId::PRIMARY)),
        "groupCollapsed" => Some(Value::Native(
            Native::ConsoleGroupCollapsed,
            RealmId::PRIMARY,
        )),
        "table" => Some(Value::Native(Native::ConsoleTable, RealmId::PRIMARY)),
        "time" => Some(Value::Native(Native::ConsoleTime, RealmId::PRIMARY)),
        "timeEnd" => Some(Value::Native(Native::ConsoleTimeEnd, RealmId::PRIMARY)),
        "timeLog" => Some(Value::Native(Native::ConsoleTimeLog, RealmId::PRIMARY)),
        "trace" => Some(Value::Native(Native::ConsoleTrace, RealmId::PRIMARY)),
        "dirxml" => Some(Value::Native(Native::ConsoleDirxml, RealmId::PRIMARY)),
        "timeStamp" => Some(Value::Native(Native::ConsoleTimeStamp, RealmId::PRIMARY)),
        "profile" => Some(Value::Native(Native::ConsoleProfile, RealmId::PRIMARY)),
        "profileEnd" => Some(Value::Native(Native::ConsoleProfileEnd, RealmId::PRIMARY)),
        key if key == WellKnown::ToStringTag.property_key() => {
            Some(Value::String("console".to_owned().into()))
        }
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeConsole => Some("console"),
        Native::ConsoleConstructor => Some("Console"),
        Native::ConsoleLog => Some("log"),
        Native::ConsoleInfo => Some("info"),
        Native::ConsoleDebug => Some("debug"),
        Native::ConsoleDir => Some("dir"),
        Native::ConsoleWarn => Some("warn"),
        Native::ConsoleError => Some("error"),
        Native::ConsoleAssert => Some("assert"),
        Native::ConsoleClear => Some("clear"),
        Native::ConsoleCount => Some("count"),
        Native::ConsoleCountReset => Some("countReset"),
        Native::ConsoleGroup => Some("group"),
        Native::ConsoleGroupEnd => Some("groupEnd"),
        Native::ConsoleGroupCollapsed => Some("groupCollapsed"),
        Native::ConsoleTable => Some("table"),
        Native::ConsoleTime => Some("time"),
        Native::ConsoleTimeEnd => Some("timeEnd"),
        Native::ConsoleTimeLog => Some("timeLog"),
        Native::ConsoleTrace => Some("trace"),
        Native::ConsoleDirxml => Some("dirxml"),
        Native::ConsoleTimeStamp => Some("timeStamp"),
        Native::ConsoleProfile => Some("profile"),
        Native::ConsoleProfileEnd => Some("profileEnd"),
        _ => None,
    }
}
