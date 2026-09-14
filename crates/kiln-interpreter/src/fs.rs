//! `node:fs` and `node:fs/promises` implementation.
//!
//! Provides filesystem operation bindings, options parsing, and property lookups.

use crate::{ExecutionError, Native, RealmId, Value};

/// Whether a Node read option requests a UTF-8 string rather than a Buffer-like byte view.
pub(crate) fn node_encoding(option: Option<&Value>) -> Result<bool, ExecutionError> {
    match option {
        None | Some(Value::Undefined) | Some(Value::Null) => Ok(false),
        Some(Value::String(encoding)) => parse_node_encoding(encoding),
        Some(Value::Object(options)) => match options.borrow().get_own("encoding") {
            None | Some(Value::Undefined) | Some(Value::Null) => Ok(false),
            Some(Value::String(encoding)) => parse_node_encoding(encoding),
            Some(value) => Err(ExecutionError::TypeErrorMessage {
                message: format!("encoding must be a string, received {}", value.type_name()),
            }),
        },
        Some(value) => Err(ExecutionError::TypeErrorMessage {
            message: format!(
                "readFile options must be a string or object, received {}",
                value.type_name()
            ),
        }),
    }
}

pub(crate) fn parse_node_encoding(encoding: &str) -> Result<bool, ExecutionError> {
    match encoding.to_ascii_lowercase().as_str() {
        "utf8" | "utf-8" => Ok(true),
        other => Err(ExecutionError::TypeErrorMessage {
            message: format!("unsupported file encoding {other:?}"),
        }),
    }
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "existsSync" => Some(Value::Native(Native::NodeFsExistsSync, RealmId::PRIMARY)),
        "readFileSync" | "writeFileSync" | "statSync" | "lstatSync" | "readdirSync"
        | "mkdirSync" | "realpathSync" | "openSync" | "readSync" | "closeSync"
        | "appendFileSync" | "unlinkSync" | "rmSync" | "renameSync" | "readlinkSync"
        | "accessSync" | "copyFileSync" => {
            Some(Value::Native(Native::NodeFsSync, RealmId::PRIMARY))
        }
        "promises" => Some(Value::Native(Native::NodeFsPromises, RealmId::PRIMARY)),
        "default" => Some(Value::Native(Native::NodeFs, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) fn promises_property(prop: &str) -> Option<Value> {
    match prop {
        "appendFile" => Some(Value::Native(Native::NodeFsAppendFile, RealmId::PRIMARY)),
        "readFile" => Some(Value::Native(Native::NodeFsReadFile, RealmId::PRIMARY)),
        "writeFile" => Some(Value::Native(Native::NodeFsWriteFile, RealmId::PRIMARY)),
        "stat" => Some(Value::Native(Native::NodeFsStat, RealmId::PRIMARY)),
        "mkdir" => Some(Value::Native(Native::NodeFsMkdir, RealmId::PRIMARY)),
        "default" => Some(Value::Native(Native::NodeFsPromises, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeFs => Some("fs"),
        Native::NodeFsPromises => Some("promises"),
        Native::NodeFsExistsSync => Some("existsSync"),
        Native::NodeFsReadFile => Some("readFile"),
        Native::NodeFsWriteFile => Some("writeFile"),
        Native::NodeFsAppendFile => Some("appendFile"),
        Native::NodeFsStat => Some("stat"),
        Native::NodeFsMkdir => Some("mkdir"),
        Native::NodeStatIsFile => Some("isFile"),
        Native::NodeStatIsDirectory => Some("isDirectory"),
        _ => None,
    }
}
