//! `node:string_decoder` implementation.
//!
//! Provides the `StringDecoder` class which decodes byte streams to strings while
//! preserving multi-byte UTF-8 sequences split across chunks.

use std::cell::RefCell;
use std::rc::Rc;

use crate::{ExecutionError, Native, NodeObject, ObjectData, RealmId, Value, tracked_object};

#[derive(Debug, Default)]
pub(crate) struct StringDecoderState {
    pub(crate) pending: Vec<u8>,
}

pub(crate) fn node_string_decoder() -> Value {
    let mut data = ObjectData {
        node_object: Some(NodeObject::StringDecoder(Rc::new(RefCell::new(
            StringDecoderState::default(),
        )))),
        ..Default::default()
    };
    data.set(
        "write",
        Value::Native(Native::NodeStringDecoderWrite, RealmId::PRIMARY),
    );
    data.set(
        "end",
        Value::Native(Native::NodeStringDecoderEnd, RealmId::PRIMARY),
    );
    Value::Object(tracked_object(data))
}

pub(crate) fn node_string_decoder_state(
    receiver: &Value,
) -> Result<Rc<RefCell<StringDecoderState>>, ExecutionError> {
    let Value::Object(object) = receiver else {
        return Err(ExecutionError::TypeErrorMessage {
            message: "StringDecoder method called on an incompatible receiver".into(),
        });
    };
    match &object.borrow().node_object {
        Some(NodeObject::StringDecoder(state)) => Ok(Rc::clone(state)),
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "StringDecoder method called on an incompatible receiver".into(),
        }),
    }
}

/// Decode every complete UTF-8 sequence, retaining only a trailing incomplete sequence. Invalid
/// interior bytes are replaced exactly as Node's decoder does; an incomplete tail is held for
/// the next write so chunk boundaries cannot change the text.
pub(crate) fn decode_utf8_chunk(pending: &mut Vec<u8>, bytes: &[u8], end: bool) -> String {
    pending.extend_from_slice(bytes);
    if end {
        return String::from_utf8_lossy(std::mem::take(pending).as_slice()).into_owned();
    }
    let mut keep = 0;
    for length in 1..=pending.len().min(3) {
        let start = pending.len() - length;
        if let Err(error) = std::str::from_utf8(&pending[start..])
            && error.valid_up_to() == 0
            && error.error_len().is_none()
        {
            keep = length;
        }
    }
    let split = pending.len() - keep;
    let tail = pending.split_off(split);
    let complete = std::mem::replace(pending, tail);
    String::from_utf8_lossy(&complete).into_owned()
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "StringDecoder" => Some(Value::Native(
            Native::NodeStringDecoderConstructor,
            RealmId::PRIMARY,
        )),
        "default" => Some(Value::Native(Native::NodeStringDecoder, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeStringDecoder => Some("string_decoder"),
        Native::NodeStringDecoderConstructor => Some("StringDecoder"),
        Native::NodeStringDecoderWrite => Some("write"),
        Native::NodeStringDecoderEnd => Some("end"),
        _ => None,
    }
}

pub(crate) fn dispatch(
    native: Native,
    receiver: &Value,
    arguments: &[Value],
) -> Result<Value, ExecutionError> {
    match native {
        Native::NodeStringDecoderWrite | Native::NodeStringDecoderEnd => {
            let state = node_string_decoder_state(receiver)?;
            let bytes = crate::node_body_bytes(arguments.first())?;
            let text = decode_utf8_chunk(
                &mut state.borrow_mut().pending,
                &bytes,
                matches!(native, Native::NodeStringDecoderEnd),
            );
            Ok(Value::String(text.into()))
        }
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "Invalid native StringDecoder operation".into(),
        }),
    }
}
