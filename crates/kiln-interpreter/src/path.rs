//! `node:path` POSIX path manipulation.
//!
//! Provides pure path operations (`join`, `resolve`, `relative`, `parse`, `format`, etc.)
//! over POSIX-style paths used by the realm.

pub use kiln_node_path::PathPart;
use kiln_node_path::{normalize_posix_path, posix_path_part, posix_path_relative};

use crate::{ExecutionError, HostOps, Native, RealmId, Value, Vm, ordinary_object};

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "join" => Some(Value::Native(Native::NodePathJoin, RealmId::PRIMARY)),
        "dirname" => Some(Value::Native(
            Native::NodePathPart(PathPart::Dir),
            RealmId::PRIMARY,
        )),
        "basename" => Some(Value::Native(
            Native::NodePathPart(PathPart::Base),
            RealmId::PRIMARY,
        )),
        "extname" => Some(Value::Native(
            Native::NodePathPart(PathPart::Ext),
            RealmId::PRIMARY,
        )),
        "normalize" => Some(Value::Native(
            Native::NodePathPart(PathPart::Normalize),
            RealmId::PRIMARY,
        )),
        "relative" => Some(Value::Native(Native::NodePathRelative, RealmId::PRIMARY)),
        "parse" => Some(Value::Native(Native::NodePathParse, RealmId::PRIMARY)),
        "format" => Some(Value::Native(Native::NodePathFormat, RealmId::PRIMARY)),
        "delimiter" => Some(Value::String(":".to_owned())),
        "posix" => Some(Value::Native(Native::NodePath, RealmId::PRIMARY)),
        "resolve" => Some(Value::Native(Native::NodePathResolve, RealmId::PRIMARY)),
        "isAbsolute" => Some(Value::Native(Native::NodePathIsAbsolute, RealmId::PRIMARY)),
        "sep" => Some(Value::String("/".to_owned())),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodePath => Some("path"),
        Native::NodePathJoin => Some("join"),
        Native::NodePathResolve => Some("resolve"),
        Native::NodePathIsAbsolute => Some("isAbsolute"),
        Native::NodePathPart(part) => match part {
            PathPart::Dir => Some("dirname"),
            PathPart::Base => Some("basename"),
            PathPart::Ext => Some("extname"),
            PathPart::Normalize => Some("normalize"),
        },
        Native::NodePathRelative => Some("relative"),
        Native::NodePathParse => Some("parse"),
        Native::NodePathFormat => Some("format"),
        _ => None,
    }
}

pub(crate) fn dispatch<H: HostOps + ?Sized>(
    vm: &mut Vm<'_, H>,
    native: Native,
    arguments: &[Value],
    instruction: usize,
) -> Result<Value, ExecutionError> {
    match native {
        Native::NodePathJoin => {
            let joined = arguments
                .iter()
                .map(Value::to_display_string)
                .collect::<Vec<_>>()
                .join("/");
            Ok(Value::String(normalize_posix_path(&joined).into()))
        }
        Native::NodePathResolve => {
            let mut resolved = String::new();
            for part in arguments.iter().map(Value::to_display_string) {
                if part.starts_with('/') {
                    resolved = part;
                } else if !part.is_empty() {
                    if !resolved.ends_with('/') {
                        resolved.push('/');
                    }
                    resolved.push_str(&part);
                }
            }
            if !resolved.starts_with('/') {
                resolved.insert(0, '/');
            }
            Ok(Value::String(normalize_posix_path(&resolved).into()))
        }
        Native::NodePathPart(part) => {
            let path = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let suffix = arguments.get(1).map(Value::to_display_string);
            Ok(Value::String(
                posix_path_part(part, &path, suffix.as_deref()).into_owned(),
            ))
        }
        Native::NodePathRelative => {
            let from = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let to = arguments
                .get(1)
                .map_or_else(String::new, Value::to_display_string);
            Ok(Value::String(posix_path_relative(&from, &to).into()))
        }
        Native::NodePathParse => {
            let path = arguments
                .first()
                .map_or_else(String::new, Value::to_display_string);
            let base = posix_path_part(PathPart::Base, &path, None).into_owned();
            let ext = posix_path_part(PathPart::Ext, &path, None).into_owned();
            let name = base
                .strip_suffix(ext.as_str())
                .filter(|_| !ext.is_empty())
                .unwrap_or(&base)
                .to_owned();
            Ok(ordinary_object([
                (
                    "root".to_owned(),
                    Value::String(if path.starts_with('/') { "/" } else { "" }.to_owned().into()),
                ),
                (
                    "dir".to_owned(),
                    Value::String(posix_path_part(PathPart::Dir, &path, None).into_owned().into()),
                ),
                ("base".to_owned(), Value::String(base.into())),
                ("ext".to_owned(), Value::String(ext.into())),
                ("name".to_owned(), Value::String(name.into())),
            ]))
        }
        // `format` is `parse` read backwards, and it prefers `base` over `name`+`ext`
        // exactly as node does -- a program that edits one field of a parsed path and hands
        // the object straight back relies on that precedence.
        Native::NodePathFormat => {
            let source = arguments.first().cloned().unwrap_or(Value::Undefined);
            let mut field = |key: &str| -> Result<String, ExecutionError> {
                Ok(match vm.read_property(&source, key, instruction)? {
                    Value::Undefined | Value::Null => String::new(),
                    other => other.to_display_string(),
                })
            };
            let directory = match field("dir")? {
                empty if empty.is_empty() => field("root")?,
                dir => dir,
            };
            let base = match field("base")? {
                empty if empty.is_empty() => {
                    format!("{}{}", field("name")?, field("ext")?)
                }
                base => base,
            };
            Ok(Value::String(if directory.is_empty() {
                base
            } else if directory == "/" {
                format!("/{base}")
            } else {
                format!("{directory}/{base}")
            }))
        }
        Native::NodePathIsAbsolute => Ok(Value::Boolean(
            arguments
                .first()
                .map(Value::to_display_string)
                .is_some_and(|path| path.starts_with('/')),
        )),
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "Invalid native path operation".into(),
        }),
    }
}
