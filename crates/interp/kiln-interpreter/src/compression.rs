//! Interpreter bridge for the private shared Rust incremental CompressionStream codec.
use std::{cell::RefCell, rc::Rc};

use crate::{ExecutionError, Value, node_bytes_value};

fn fail(message: &str) -> ExecutionError {
    ExecutionError::TypeErrorMessage {
        message: message.to_owned(),
    }
}

pub(crate) fn primitive(arguments: &[Value]) -> Result<Value, ExecutionError> {
    let Some(Value::Object(object)) = arguments.first() else {
        return Err(fail("Invalid CompressionStream receiver"));
    };
    let Some(Value::Number(operation)) = arguments.get(1) else {
        return Err(fail("Invalid CompressionStream operation"));
    };
    match *operation as u32 {
        0 => {
            let format = arguments.get(2).map(Value::as_number).unwrap_or(0.0) as u32;
            let direction = arguments.get(3).map(Value::as_number).unwrap_or(0.0) as u32;
            let format = match format {
                1 => kiln_compression::Format::Gzip,
                2 => kiln_compression::Format::Deflate,
                3 => kiln_compression::Format::DeflateRaw,
                _ => return Err(fail("Unsupported compression format")),
            };
            let direction = match direction {
                1 => kiln_compression::Direction::Compress,
                2 => kiln_compression::Direction::Decompress,
                _ => return Err(fail("Invalid compression direction")),
            };
            object.borrow_mut().compression = Some(Rc::new(RefCell::new(
                kiln_compression::Codec::new(format, direction),
            )));
            Ok(Value::Undefined)
        }
        1 => {
            let codec = object
                .borrow()
                .compression
                .clone()
                .ok_or_else(|| fail("Invalid CompressionStream receiver"))?;
            let Some(Value::TypedArray(bytes)) = arguments.get(2) else {
                return Err(fail("CompressionStream chunks must be Uint8Array values"));
            };
            let finish = matches!(arguments.get(3), Some(Value::Boolean(true)));
            let output = codec
                .borrow_mut()
                .push(&bytes.bytes(), finish)
                .map_err(|_| fail("Invalid compressed data"))?;
            Ok(node_bytes_value(output))
        }
        _ => Err(fail("Invalid CompressionStream operation")),
    }
}
