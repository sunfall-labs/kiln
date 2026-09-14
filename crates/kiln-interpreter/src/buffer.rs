//! `node:buffer` and global `Buffer` implementation.
//!
//! Provides `Buffer` constructor, statics (`from`, `alloc`, `isBuffer`, etc.),
//! and Uint8Array `toString` encoding conversions.

use std::cell::RefCell;
use std::rc::Rc;

use crate::{
    bounded_buffer, ordinary_object, BufferData, ExecutionError, Native, ObjectData, RealmId,
    TypedArrayData, TypedKind, Value,
};

/// Constructs a `Buffer` (`Uint8Array` of `TypedKind::Uint8`) from a raw byte vector.
pub(crate) fn node_bytes_value(bytes: Vec<u8>) -> Value {
    let length = bytes.len();
    let buffer = BufferData::owned(bytes);
    Value::TypedArray(Rc::new(TypedArrayData {
        buffer,
        offset: 0,
        length: Some(length),
        kind: TypedKind::Uint8,
        properties: RefCell::new(ObjectData::default()),
    }))
}

/// Static property lookup for `Buffer` and `node:buffer`.
pub(crate) fn property(native: Native, prop: &str) -> Option<Value> {
    match prop {
        "from" => Some(Value::Native(Native::NodeBufferFrom, RealmId::PRIMARY)),
        "alloc" | "allocUnsafe" | "allocUnsafeSlow" => {
            Some(Value::Native(Native::NodeBufferAlloc, RealmId::PRIMARY))
        }
        "isBuffer" => Some(Value::Native(Native::NodeBufferIsBuffer, RealmId::PRIMARY)),
        "byteLength" => Some(Value::Native(
            Native::NodeBufferByteLength,
            RealmId::PRIMARY,
        )),
        "concat" => Some(Value::Native(Native::NodeBufferConcat, RealmId::PRIMARY)),
        // `require("buffer").Buffer` is the constructor, and has to be the *same* value as
        // the global `Buffer` -- that identity is what every `Buffer.isBuffer` shim relies
        // on. `default` stays the namespace: an ESM default import of a Node builtin is the
        // namespace object, not the constructor.
        "Buffer" => Some(Value::Native(Native::NodeBufferGlobal, RealmId::PRIMARY)),
        "default" if matches!(native, Native::NodeBuffer) => {
            Some(Value::Native(Native::NodeBuffer, RealmId::PRIMARY))
        }
        "constants" => Some(ordinary_object([(
            "MAX_STRING_LENGTH".to_owned(),
            Value::Number(536_870_888.0),
        )])),
        _ => None,
    }
}

/// Executes machine-dispatched `Native::NodeBuffer*` calls.
pub(crate) fn dispatch(
    native: Native,
    receiver: &Value,
    arguments: &[Value],
) -> Result<Value, ExecutionError> {
    match native {
        Native::NodeBufferFrom => {
            let bytes = match arguments.first().unwrap_or(&Value::Undefined) {
                Value::String(text) => text
                    .to_scalar_string()
                    .map(|value| value.into_bytes())
                    .map_err(|_| ExecutionError::TypeErrorMessage {
                        message: "Buffer.from string contains a lone surrogate".to_owned(),
                    })?,
                Value::TypedArray(view) => view.bytes(),
                Value::Array(items) => items
                    .borrow()
                    .iter()
                    .map(|value| value.as_number() as u8)
                    .collect(),
                value => {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: format!(
                            "The first argument must be a string, Buffer, Uint8Array, or Array; received {}",
                            value.type_name()
                        ),
                    });
                }
            };
            Ok(node_bytes_value(bytes))
        }
        Native::NodeBufferAlloc => {
            let length = bounded_buffer(
                arguments.first().map(Value::as_number).unwrap_or(f64::NAN),
                1,
            )?;
            let fill = arguments.get(1).map(Value::as_number).unwrap_or(0.0) as u8;
            Ok(node_bytes_value(vec![fill; length]))
        }
        Native::NodeBufferIsBuffer => Ok(Value::Boolean(matches!(
            arguments.first(),
            Some(Value::TypedArray(view)) if view.kind == TypedKind::Uint8
        ))),
        Native::NodeBufferByteLength => {
            let length = match arguments.first().unwrap_or(&Value::Undefined) {
                Value::String(text) => text.len(),
                Value::TypedArray(view) => view.bytes().len(),
                Value::ArrayBuffer(buffer) => buffer.byte_length(),
                value => value.to_display_string().len(),
            };
            Ok(Value::Number(length as f64))
        }
        Native::NodeBufferConcat => {
            let Some(Value::Array(items)) = arguments.first() else {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "The \"list\" argument must be an instance of Array".into(),
                });
            };
            let mut bytes = Vec::new();
            for (index, item) in items.borrow().iter().enumerate() {
                let Value::TypedArray(view) = item else {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: format!(
                            "The \"list[{index}]\" argument must be an instance of Buffer or Uint8Array"
                        ),
                    });
                };
                bytes.extend(view.bytes());
            }
            if let Some(length) = arguments.get(1) {
                let length = bounded_buffer(length.as_number(), 1)?;
                bytes.resize(length, 0);
                bytes.truncate(length);
            }
            Ok(node_bytes_value(bytes))
        }
        Native::NodeBufferToString => {
            let Value::TypedArray(view) = receiver else {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Buffer.toString called on an incompatible receiver".into(),
                });
            };
            let encoding = arguments
                .first()
                .map(Value::to_display_string)
                .unwrap_or_else(|| "utf8".into())
                .to_ascii_lowercase();
            let bytes = view.bytes();
            match encoding.as_str() {
                "utf8" | "utf-8" => Ok(Value::String(
                    String::from_utf8_lossy(&bytes).into_owned().into(),
                )),
                "hex" => {
                    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
                    let mut hex_str = String::with_capacity(bytes.len() * 2);
                    for &b in &bytes {
                        hex_str.push(HEX_DIGITS[(b >> 4) as usize] as char);
                        hex_str.push(HEX_DIGITS[(b & 0x0f) as usize] as char);
                    }
                    Ok(Value::String(hex_str.into()))
                }
                "base64" => Ok(Value::String(
                    crate::to_base64_text(&bytes, false, false).into(),
                )),
                "base64url" => Ok(Value::String(
                    crate::to_base64_text(&bytes, true, true).into(),
                )),
                "latin1" | "binary" | "ascii" => {
                    let (decoded, _) =
                        encoding_rs::WINDOWS_1252.decode_without_bom_handling(&bytes);
                    Ok(Value::String(decoded.into_owned().into()))
                }
                "utf16le" | "utf-16le" | "ucs2" | "ucs-2" => {
                    let (decoded, _) = encoding_rs::UTF_16LE.decode_without_bom_handling(&bytes);
                    Ok(Value::String(decoded.into_owned().into()))
                }
                other => {
                    if let Some(enc) = encoding_rs::Encoding::for_label(other.as_bytes()) {
                        let (decoded, _) = enc.decode_without_bom_handling(&bytes);
                        Ok(Value::String(decoded.into_owned().into()))
                    } else {
                        Err(ExecutionError::TypeErrorMessage {
                            message: format!("unsupported Buffer encoding: {encoding}"),
                        })
                    }
                }
            }
        }
        _ => unreachable!(),
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeBuffer => Some("buffer"),
        Native::NodeBufferGlobal => Some("Buffer"),
        Native::NodeBufferFrom => Some("from"),
        Native::NodeBufferAlloc => Some("alloc"),
        Native::NodeBufferIsBuffer => Some("isBuffer"),
        Native::NodeBufferByteLength => Some("byteLength"),
        Native::NodeBufferConcat => Some("concat"),
        Native::NodeBufferToString => Some("toString"),
        _ => None,
    }
}
