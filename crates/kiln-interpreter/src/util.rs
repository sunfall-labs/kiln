//! `node:util` implementation.
//!
//! Provides utility functions including string formatting (`format`), inheritance helpers (`inherits`),
//! inspection (`inspect`), and debug logging (`debuglog`).

use crate::{ExecutionError, Native, RealmId, Value, is_callable};

/// `node:util` property resolution.
pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "default" => Some(Value::Native(Native::NodeUtil, RealmId::PRIMARY)),
        "inherits" => Some(Value::Native(Native::NodeUtilInherits, RealmId::PRIMARY)),
        "debuglog" => Some(Value::Native(Native::NodeUtilDebuglog, RealmId::PRIMARY)),
        "deprecate" => Some(Value::Native(Native::NodeUtilDeprecate, RealmId::PRIMARY)),
        "inspect" => Some(Value::Native(Native::NodeUtilInspect, RealmId::PRIMARY)),
        "format" => Some(Value::Native(Native::NodeUtilFormat, RealmId::PRIMARY)),
        "TextEncoder" => Some(Value::Native(
            Native::TextEncoderConstructor,
            RealmId::PRIMARY,
        )),
        "TextDecoder" => Some(Value::Native(
            Native::TextDecoderConstructor,
            RealmId::PRIMARY,
        )),
        _ => None,
    }
}

/// JS name mapping for `node:util` natives.
pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeUtil => Some("util"),
        Native::NodeUtilInherits => Some("inherits"),
        Native::NodeUtilDebuglog => Some("debuglog"),
        Native::NodeUtilDebugNoop => Some("debuglog"),
        Native::NodeUtilDeprecate => Some("deprecate"),
        Native::NodeUtilInspect => Some("inspect"),
        Native::NodeUtilFormat => Some("format"),
        _ => None,
    }
}

/// Implementation of `util.format(...)`.
pub(crate) fn format(arguments: &[Value]) -> String {
    let Some(first) = arguments.first() else {
        return String::new();
    };
    let Value::String(template) = first else {
        return arguments
            .iter()
            .map(crate::inspect)
            .collect::<Vec<_>>()
            .join(" ");
    };
    let mut out = String::new();
    let mut values = arguments.iter().skip(1);
    let mut chars = template.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '%' {
            out.push(character);
            continue;
        }
        let Some(specifier) = chars.peek().copied() else {
            out.push('%');
            break;
        };
        if specifier == '%' {
            chars.next();
            out.push('%');
            continue;
        }
        let Some(value) = values.next() else {
            out.push('%');
            continue;
        };
        chars.next();
        match specifier {
            's' => out.push_str(&value.to_display_string()),
            'd' | 'i' => out.push_str(&crate::number_string(value.as_number())),
            'j' => out.push_str(&crate::inspect(value)),
            'o' | 'O' => out.push_str(&crate::inspect(value)),
            'f' => out.push_str(&value.as_number().to_string()),
            _ => {
                out.push('%');
                out.push(specifier);
            }
        }
    }
    for value in values {
        out.push(' ');
        out.push_str(&crate::inspect(value));
    }
    out
}

/// Implementation of `util.inherits(constructor, superConstructor)`.
pub(crate) fn inherits<F>(
    arguments: &[Value],
    mut read_property: F,
) -> Result<Value, ExecutionError>
where
    F: FnMut(&Value, &str) -> Result<Value, ExecutionError>,
{
    let constructor = arguments.first().cloned().unwrap_or(Value::Undefined);
    let super_constructor = arguments.get(1).cloned().unwrap_or(Value::Undefined);
    if !is_callable(&constructor) || !is_callable(&super_constructor) {
        return Err(ExecutionError::TypeErrorMessage {
            message: "The constructor and super constructor must be functions".to_owned(),
        });
    }
    let super_prototype = read_property(&super_constructor, "prototype")?;
    let constructor_prototype = read_property(&constructor, "prototype")?;
    match &constructor_prototype {
        Value::Object(object) => {
            object.borrow_mut().set_prototype(super_prototype);
        }
        Value::Function(function) => {
            function
                .properties
                .borrow_mut()
                .set_prototype(super_prototype);
        }
        _ => {}
    }
    Ok(Value::Undefined)
}

/// Dispatch for `node:util` native functions.
pub(crate) fn dispatch<F>(
    native: Native,
    arguments: &[Value],
    read_property: F,
) -> Result<Value, ExecutionError>
where
    F: FnMut(&Value, &str) -> Result<Value, ExecutionError>,
{
    match native {
        Native::NodeUtilDebuglog => Ok(Value::Native(Native::NodeUtilDebugNoop, RealmId::PRIMARY)),
        Native::NodeUtilDebugNoop => Ok(Value::Undefined),
        // Warning delivery is an embedding policy. A deterministic realm preserves the
        // wrapped callable and performs no ambient stderr write.
        Native::NodeUtilDeprecate => Ok(arguments.first().cloned().unwrap_or(Value::Undefined)),
        Native::NodeUtilInspect => Ok(Value::String(crate::inspect(
            arguments.first().unwrap_or(&Value::Undefined),
        ))),
        Native::NodeUtilFormat => Ok(Value::String(format(arguments))),
        Native::NodeUtilInherits => inherits(arguments, read_property),
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "Invalid native util operation".into(),
        }),
    }
}
