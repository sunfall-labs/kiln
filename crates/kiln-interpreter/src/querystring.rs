//! `node:querystring` query string parsing and decoding.

use std::cell::RefCell;
use std::rc::Rc;

use crate::{ArrayData, ExecutionError, Native, ObjectData, RealmId, Value};

/// Decodes percent-encoded bytes and converts '+' into space, as required by
/// URL application/x-www-form-urlencoded component decoding.
pub(crate) fn decode_form_component(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let hex = |byte: u8| match byte {
                    b'0'..=b'9' => Some(byte - b'0'),
                    b'a'..=b'f' => Some(byte - b'a' + 10),
                    b'A'..=b'F' => Some(byte - b'A' + 10),
                    _ => None,
                };
                if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                    decoded.push((high << 4) | low);
                    index += 2;
                } else {
                    decoded.push(bytes[index]);
                }
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Parses a URL query string into an object of key-value pairs or arrays.
pub(crate) fn parse(input: &str) -> Value {
    let mut object = ObjectData::default();
    for part in input.split('&').filter(|part| !part.is_empty()) {
        let (name, value) = part.split_once('=').unwrap_or((part, ""));
        let name = decode_form_component(name);
        let value = Value::String(decode_form_component(value));
        match object.get_own(&name).cloned() {
            None => object.set(&name, value),
            Some(Value::Array(values)) => values.borrow_mut().push(value),
            Some(previous) => {
                object.set(&name, Value::Array(ArrayData::new(vec![previous, value])))
            }
        }
    }
    Value::Object(Rc::new(RefCell::new(object)))
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "parse" | "decode" => Some(Value::Native(
            Native::NodeQueryStringParse,
            RealmId::PRIMARY,
        )),
        "default" => Some(Value::Native(Native::NodeQueryString, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeQueryString => Some("querystring"),
        Native::NodeQueryStringParse => Some("parse"),
        _ => None,
    }
}

pub(crate) fn dispatch(native: Native, arguments: &[Value]) -> Result<Value, ExecutionError> {
    match native {
        Native::NodeQueryStringParse => Ok(parse(
            &arguments
                .first()
                .map(Value::to_display_string)
                .unwrap_or_default(),
        )),
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "Invalid native querystring operation".into(),
        }),
    }
}
