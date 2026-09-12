//! `node:zlib` compression and decompression primitives.
//!
//! Handles Gzip, Gunzip, Deflate, Inflate, and Unzip transforms and synchronous operations,
//! wrapping the streaming transform abstractions around `flate2`.

use std::cell::RefCell;
use std::rc::Rc;

use kiln_host_abi::HostOps;

use crate::{
    ExecutionError, Native, NodeObject, ObjectData, PrototypeTable, ReadableState, RealmId, Value,
    Vm, WritableState, new_readable_state, node_bytes_value, ordinary_object,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ZlibTransformKind {
    Gzip,
    Gunzip,
    Deflate,
    Inflate,
    Unzip,
    Brotli,
}

#[derive(Debug)]
pub(crate) struct ZlibState {
    pub(crate) kind: ZlibTransformKind,
    pub(crate) readable: Rc<RefCell<ReadableState>>,
    pub(crate) writable: Rc<RefCell<WritableState>>,
    pub(crate) accumulated: RefCell<Vec<u8>>,
}

pub(crate) fn zlib_buffer_arg(value: Option<&Value>) -> Vec<u8> {
    match value {
        Some(Value::TypedArray(view)) => view.bytes(),
        Some(Value::ArrayBuffer(ab)) => ab.bytes.borrow().clone(),
        Some(Value::String(s)) => s.as_bytes().to_vec(),
        _ => Vec::new(),
    }
}

pub(crate) fn transform_zlib(
    kind: ZlibTransformKind,
    bytes: &[u8],
) -> Result<Vec<u8>, std::io::Error> {
    use std::io::{Read, Write};
    match kind {
        ZlibTransformKind::Gzip => {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(bytes)?;
            encoder.finish()
        }
        ZlibTransformKind::Gunzip => {
            let mut decoder = flate2::read::GzDecoder::new(bytes);
            let mut out = Vec::new();
            decoder.read_to_end(&mut out)?;
            Ok(out)
        }
        ZlibTransformKind::Deflate => {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(bytes)?;
            encoder.finish()
        }
        ZlibTransformKind::Inflate => {
            let mut decoder = flate2::read::ZlibDecoder::new(bytes);
            let mut out = Vec::new();
            match decoder.read_to_end(&mut out) {
                Ok(_) => Ok(out),
                Err(_) => {
                    let mut raw = flate2::read::DeflateDecoder::new(bytes);
                    let mut raw_out = Vec::new();
                    raw.read_to_end(&mut raw_out)?;
                    Ok(raw_out)
                }
            }
        }
        ZlibTransformKind::Unzip => {
            let mut gz = flate2::read::GzDecoder::new(bytes);
            let mut out = Vec::new();
            if gz.read_to_end(&mut out).is_ok() {
                Ok(out)
            } else {
                let mut zlib = flate2::read::ZlibDecoder::new(bytes);
                let mut zout = Vec::new();
                if zlib.read_to_end(&mut zout).is_ok() {
                    Ok(zout)
                } else {
                    let mut raw = flate2::read::DeflateDecoder::new(bytes);
                    let mut raw_out = Vec::new();
                    raw.read_to_end(&mut raw_out)?;
                    Ok(raw_out)
                }
            }
        }
        ZlibTransformKind::Brotli => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "brotli decompression not yet supported",
        )),
    }
}

pub(crate) fn node_zlib_create_transform(kind: ZlibTransformKind) -> Value {
    let readable_state = new_readable_state(16 * 1024, None);
    let writable_state = Rc::new(RefCell::new(WritableState {
        ended: false,
        finished: false,
        destroyed: false,
        write: None,
    }));
    let zlib_state = Rc::new(ZlibState {
        kind,
        readable: Rc::clone(&readable_state),
        writable: Rc::clone(&writable_state),
        accumulated: RefCell::new(Vec::new()),
    });
    let object = Rc::new(RefCell::new(ObjectData::default()));
    {
        let mut data = object.borrow_mut();
        data.node_events.get_or_insert_with(Vec::new);
        data.node_object = Some(NodeObject::Zlib(zlib_state));
        data.set_prototype(Value::Native(
            Native::PrototypeOf(PrototypeTable::Readable),
            RealmId::PRIMARY,
        ));
        data.set_non_enumerable(
            "write",
            Value::Native(Native::NodeWritableWrite, RealmId::PRIMARY),
        );
        data.set_non_enumerable(
            "end",
            Value::Native(Native::NodeWritableEnd, RealmId::PRIMARY),
        );
        data.set_non_enumerable("writableEnded", Value::Boolean(false));
        data.set_non_enumerable("writableFinished", Value::Boolean(false));
        data.set_non_enumerable("readableEnded", Value::Boolean(false));
        data.set_non_enumerable("destroyed", Value::Boolean(false));
    }
    Value::Object(object)
}

pub(crate) fn constants() -> Value {
    ordinary_object([
        ("Z_NO_FLUSH".to_owned(), Value::Number(0.0)),
        ("Z_PARTIAL_FLUSH".to_owned(), Value::Number(1.0)),
        ("Z_SYNC_FLUSH".to_owned(), Value::Number(2.0)),
        ("Z_FULL_FLUSH".to_owned(), Value::Number(3.0)),
        ("Z_FINISH".to_owned(), Value::Number(4.0)),
        ("Z_BLOCK".to_owned(), Value::Number(5.0)),
        ("Z_TREES".to_owned(), Value::Number(6.0)),
        ("Z_OK".to_owned(), Value::Number(0.0)),
        ("Z_STREAM_END".to_owned(), Value::Number(1.0)),
        ("Z_NEED_DICT".to_owned(), Value::Number(2.0)),
        ("Z_ERRNO".to_owned(), Value::Number(-1.0)),
        ("Z_STREAM_ERROR".to_owned(), Value::Number(-2.0)),
        ("Z_DATA_ERROR".to_owned(), Value::Number(-3.0)),
        ("Z_MEM_ERROR".to_owned(), Value::Number(-4.0)),
        ("Z_BUF_ERROR".to_owned(), Value::Number(-5.0)),
        ("Z_VERSION_ERROR".to_owned(), Value::Number(-6.0)),
    ])
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "createInflate" => Some(Value::Native(
            Native::NodeZlibCreateInflate,
            RealmId::PRIMARY,
        )),
        "createGunzip" => Some(Value::Native(
            Native::NodeZlibCreateGunzip,
            RealmId::PRIMARY,
        )),
        "createGzip" => Some(Value::Native(Native::NodeZlibCreateGzip, RealmId::PRIMARY)),
        "createDeflate" => Some(Value::Native(
            Native::NodeZlibCreateDeflate,
            RealmId::PRIMARY,
        )),
        "createUnzip" => Some(Value::Native(Native::NodeZlibCreateUnzip, RealmId::PRIMARY)),
        "createBrotliDecompress" => Some(Value::Native(
            Native::NodeZlibCreateBrotliDecompress,
            RealmId::PRIMARY,
        )),
        "gunzipSync" => Some(Value::Native(Native::NodeZlibGunzipSync, RealmId::PRIMARY)),
        "inflateSync" => Some(Value::Native(Native::NodeZlibInflateSync, RealmId::PRIMARY)),
        "gzipSync" => Some(Value::Native(Native::NodeZlibGzipSync, RealmId::PRIMARY)),
        "deflateSync" => Some(Value::Native(Native::NodeZlibDeflateSync, RealmId::PRIMARY)),
        "unzipSync" => Some(Value::Native(Native::NodeZlibUnzipSync, RealmId::PRIMARY)),
        "constants" => Some(constants()),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeZlibCreateGzip => Some("createGzip"),
        Native::NodeZlibCreateGunzip => Some("createGunzip"),
        Native::NodeZlibCreateDeflate => Some("createDeflate"),
        Native::NodeZlibCreateInflate => Some("createInflate"),
        Native::NodeZlibCreateUnzip => Some("createUnzip"),
        Native::NodeZlibCreateBrotliDecompress => Some("createBrotliDecompress"),
        Native::NodeZlibCreateDecompress => Some("createUnzip"),
        Native::NodeZlibGzipSync => Some("gzipSync"),
        Native::NodeZlibGunzipSync => Some("gunzipSync"),
        Native::NodeZlibDeflateSync => Some("deflateSync"),
        Native::NodeZlibInflateSync => Some("inflateSync"),
        Native::NodeZlibUnzipSync => Some("unzipSync"),
        _ => None,
    }
}

impl<H: HostOps + ?Sized> Vm<'_, H> {
    pub(crate) fn zlib_dispatch(
        &mut self,
        native: Native,
        _receiver: &Value,
        arguments: &[Value],
        _instruction: usize,
    ) -> Result<Value, ExecutionError> {
        match native {
            Native::NodeZlibCreateGzip => Ok(node_zlib_create_transform(ZlibTransformKind::Gzip)),
            Native::NodeZlibCreateGunzip => {
                Ok(node_zlib_create_transform(ZlibTransformKind::Gunzip))
            }
            Native::NodeZlibCreateDeflate => {
                Ok(node_zlib_create_transform(ZlibTransformKind::Deflate))
            }
            Native::NodeZlibCreateInflate => {
                Ok(node_zlib_create_transform(ZlibTransformKind::Inflate))
            }
            Native::NodeZlibCreateUnzip => Ok(node_zlib_create_transform(ZlibTransformKind::Unzip)),
            Native::NodeZlibCreateBrotliDecompress => {
                Ok(node_zlib_create_transform(ZlibTransformKind::Brotli))
            }
            Native::NodeZlibCreateDecompress => {
                Ok(node_zlib_create_transform(ZlibTransformKind::Gunzip))
            }
            Native::NodeZlibGzipSync => {
                let bytes = zlib_buffer_arg(arguments.first());
                let res = transform_zlib(ZlibTransformKind::Gzip, &bytes).map_err(|err| {
                    ExecutionError::TypeErrorMessage {
                        message: err.to_string(),
                    }
                })?;
                Ok(node_bytes_value(res))
            }
            Native::NodeZlibGunzipSync => {
                let bytes = zlib_buffer_arg(arguments.first());
                let res = transform_zlib(ZlibTransformKind::Gunzip, &bytes).map_err(|err| {
                    ExecutionError::TypeErrorMessage {
                        message: err.to_string(),
                    }
                })?;
                Ok(node_bytes_value(res))
            }
            Native::NodeZlibDeflateSync => {
                let bytes = zlib_buffer_arg(arguments.first());
                let res = transform_zlib(ZlibTransformKind::Deflate, &bytes).map_err(|err| {
                    ExecutionError::TypeErrorMessage {
                        message: err.to_string(),
                    }
                })?;
                Ok(node_bytes_value(res))
            }
            Native::NodeZlibInflateSync => {
                let bytes = zlib_buffer_arg(arguments.first());
                let res = transform_zlib(ZlibTransformKind::Inflate, &bytes).map_err(|err| {
                    ExecutionError::TypeErrorMessage {
                        message: err.to_string(),
                    }
                })?;
                Ok(node_bytes_value(res))
            }
            Native::NodeZlibUnzipSync => {
                let bytes = zlib_buffer_arg(arguments.first());
                let res = transform_zlib(ZlibTransformKind::Unzip, &bytes).map_err(|err| {
                    ExecutionError::TypeErrorMessage {
                        message: err.to_string(),
                    }
                })?;
                Ok(node_bytes_value(res))
            }
            _ => Err(ExecutionError::TypeErrorMessage {
                message: "Invalid native zlib operation".into(),
            }),
        }
    }
}
