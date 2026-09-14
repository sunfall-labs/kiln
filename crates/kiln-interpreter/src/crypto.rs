//! `node:crypto` and global Web Crypto primitives.
//!
//! This module houses cryptographic hashing, HMAC, secure random generation,
//! and constant-time comparison primitives, backed by RustCrypto (`digest`, `md-5`,
//! `sha1`, `sha2`) and the VM's deterministic entropy engine.
//!
//! Kept in a dedicated module alongside `temporal.rs` and `intl.rs` to prevent
//! domain-specific cryptographic logic from burying the bytecode interpreter.

use std::cell::RefCell;
use std::rc::Rc;

use digest::Digest;
use kiln_host_abi::HostOps;

use crate::{
    ArrayData, ExecutionError, Native, NodeObject, ObjectData, RealmId, TypedKind, Value, Vm,
    is_callable, node_body_bytes, node_bytes_value,
};

#[derive(Clone, Copy, Debug)]
pub(crate) enum HashAlgorithm {
    Md5,
    Sha1,
    Sha256,
    Sha512,
}

#[derive(Debug)]
pub(crate) struct HashState {
    pub(crate) algorithm: HashAlgorithm,
    pub(crate) hmac_key: Option<Vec<u8>>,
    pub(crate) bytes: Vec<u8>,
    pub(crate) digested: bool,
}

pub(crate) fn view_or_buffer_bytes(val: &Value) -> Result<Vec<u8>, ExecutionError> {
    match val {
        Value::TypedArray(view) => Ok(view.bytes()),
        Value::ArrayBuffer(buffer) => Ok(buffer.bytes.borrow().clone()),
        Value::DataView(view) => {
            if view.out_of_bounds() {
                return Ok(Vec::new());
            }
            let length = view.byte_length().unwrap_or(0);
            Ok(view.buffer.bytes.borrow()[view.offset..view.offset + length].to_vec())
        }
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "The \"a\" and \"b\" arguments must be of type ArrayBufferView or ArrayBuffer"
                .into(),
        }),
    }
}

pub(crate) fn node_hash(arguments: &[Value]) -> Result<Value, ExecutionError> {
    let name = arguments
        .first()
        .map(Value::to_display_string)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace('-', "");
    let algorithm = match name.as_str() {
        "md5" => HashAlgorithm::Md5,
        "sha1" => HashAlgorithm::Sha1,
        "sha256" => HashAlgorithm::Sha256,
        "sha512" => HashAlgorithm::Sha512,
        _ => {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("Digest method not supported: {name}"),
            });
        }
    };
    let mut data = ObjectData {
        node_object: Some(NodeObject::Hash(Rc::new(RefCell::new(HashState {
            algorithm,
            hmac_key: None,
            bytes: Vec::new(),
            digested: false,
        })))),
        ..Default::default()
    };
    data.set(
        "update",
        Value::Native(Native::NodeHashUpdate, RealmId::PRIMARY),
    );
    data.set(
        "digest",
        Value::Native(Native::NodeHashDigest, RealmId::PRIMARY),
    );
    Ok(Value::Object(Rc::new(RefCell::new(data))))
}

pub(crate) fn node_hmac(arguments: &[Value]) -> Result<Value, ExecutionError> {
    let name = arguments
        .first()
        .map(Value::to_display_string)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace('-', "");
    let algorithm = match name.as_str() {
        "md5" => HashAlgorithm::Md5,
        "sha1" => HashAlgorithm::Sha1,
        "sha256" => HashAlgorithm::Sha256,
        "sha512" => HashAlgorithm::Sha512,
        _ => {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!("Digest method not supported: {name}"),
            });
        }
    };
    let Some(key_arg) = arguments.get(1) else {
        return Err(ExecutionError::TypeErrorMessage {
            message:
                "The \"key\" argument must be of type string or an instance of Buffer or TypedArray"
                    .into(),
        });
    };
    if key_arg.is_nullish() {
        return Err(ExecutionError::TypeErrorMessage {
            message:
                "The \"key\" argument must be of type string or an instance of Buffer or TypedArray"
                    .into(),
        });
    }
    let key = node_body_bytes(Some(key_arg))?;
    let mut data = ObjectData {
        node_object: Some(NodeObject::Hash(Rc::new(RefCell::new(HashState {
            algorithm,
            hmac_key: Some(key),
            bytes: Vec::new(),
            digested: false,
        })))),
        ..Default::default()
    };
    data.set(
        "update",
        Value::Native(Native::NodeHashUpdate, RealmId::PRIMARY),
    );
    data.set(
        "digest",
        Value::Native(Native::NodeHashDigest, RealmId::PRIMARY),
    );
    Ok(Value::Object(Rc::new(RefCell::new(data))))
}

pub(crate) fn node_hash_state(receiver: &Value) -> Result<Rc<RefCell<HashState>>, ExecutionError> {
    let Value::Object(object) = receiver else {
        return Err(ExecutionError::TypeErrorMessage {
            message: "Hash method called on an incompatible receiver".into(),
        });
    };
    match &object.borrow().node_object {
        Some(NodeObject::Hash(state)) => Ok(Rc::clone(state)),
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "Hash method called on an incompatible receiver".into(),
        }),
    }
}

pub(crate) fn node_hash_digest(algorithm: HashAlgorithm, bytes: &[u8]) -> Vec<u8> {
    match algorithm {
        HashAlgorithm::Md5 => md5::Md5::digest(bytes).to_vec(),
        HashAlgorithm::Sha1 => sha1::Sha1::digest(bytes).to_vec(),
        HashAlgorithm::Sha256 => sha2::Sha256::digest(bytes).to_vec(),
        HashAlgorithm::Sha512 => sha2::Sha512::digest(bytes).to_vec(),
    }
}

pub(crate) fn node_hmac_digest(algorithm: HashAlgorithm, key: &[u8], data: &[u8]) -> Vec<u8> {
    let block_size = match algorithm {
        HashAlgorithm::Md5 | HashAlgorithm::Sha1 | HashAlgorithm::Sha256 => 64,
        HashAlgorithm::Sha512 => 128,
    };
    let mut key_block = vec![0_u8; block_size];
    if key.len() > block_size {
        let hashed_key = node_hash_digest(algorithm, key);
        key_block[..hashed_key.len()].copy_from_slice(&hashed_key);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner_input = Vec::with_capacity(block_size + data.len());
    for &b in &key_block {
        inner_input.push(b ^ 0x36);
    }
    inner_input.extend_from_slice(data);
    let inner_hash = node_hash_digest(algorithm, &inner_input);

    let mut outer_input = Vec::with_capacity(block_size + inner_hash.len());
    for &b in &key_block {
        outer_input.push(b ^ 0x5c);
    }
    outer_input.extend_from_slice(&inner_hash);
    node_hash_digest(algorithm, &outer_input)
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "default" => Some(Value::Native(Native::NodeCrypto, RealmId::PRIMARY)),
        "randomUUID" => Some(Value::Native(
            Native::NodeCryptoRandomUuid,
            RealmId::PRIMARY,
        )),
        "randomBytes" => Some(Value::Native(
            Native::NodeCryptoRandomBytes,
            RealmId::PRIMARY,
        )),
        "randomFillSync" => Some(Value::Native(
            Native::NodeCryptoRandomFillSync,
            RealmId::PRIMARY,
        )),
        "getHashes" => Some(Value::Native(Native::NodeCryptoGetHashes, RealmId::PRIMARY)),
        "getRandomValues" => Some(Value::Native(
            Native::NodeCryptoGetRandomValues,
            RealmId::PRIMARY,
        )),
        "createHash" | "Hash" => Some(Value::Native(
            Native::NodeCryptoCreateHash,
            RealmId::PRIMARY,
        )),
        "createHmac" | "Hmac" => Some(Value::Native(
            Native::NodeCryptoCreateHmac,
            RealmId::PRIMARY,
        )),
        "timingSafeEqual" => Some(Value::Native(
            Native::NodeCryptoTimingSafeEqual,
            RealmId::PRIMARY,
        )),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeCryptoRandomUuid => Some("randomUUID"),
        Native::NodeCryptoRandomBytes => Some("randomBytes"),
        Native::NodeCryptoRandomFillSync => Some("randomFillSync"),
        Native::NodeCryptoGetHashes => Some("getHashes"),
        Native::NodeCryptoGetRandomValues => Some("getRandomValues"),
        Native::NodeCryptoCreateHash => Some("createHash"),
        Native::NodeCryptoCreateHmac => Some("createHmac"),
        Native::NodeCryptoTimingSafeEqual => Some("timingSafeEqual"),
        Native::NodeHashUpdate => Some("update"),
        Native::NodeHashDigest => Some("digest"),
        _ => None,
    }
}

impl<H: HostOps + ?Sized> Vm<'_, H> {
    pub(crate) fn crypto_dispatch(
        &mut self,
        native: Native,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        match native {
            Native::NodeCryptoRandomUuid => {
                let first = self.entropy_word(instruction)?.to_be_bytes();
                let second = self.entropy_word(instruction)?.to_be_bytes();
                let mut bytes = [0_u8; 16];
                bytes[..8].copy_from_slice(&first);
                bytes[8..].copy_from_slice(&second);
                // RFC 9562 UUIDv4: the realm entropy supplies payload bits; version and variant
                // are structural. Deterministic/seeded realms reproduce, host realms will draw
                // their words through HostOp::EntropyU64 once that execution path is enabled.
                bytes[6] = (bytes[6] & 0x0f) | 0x40;
                bytes[8] = (bytes[8] & 0x3f) | 0x80;
                Ok(Value::String(format!(
                    "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                    bytes[0],
                    bytes[1],
                    bytes[2],
                    bytes[3],
                    bytes[4],
                    bytes[5],
                    bytes[6],
                    bytes[7],
                    bytes[8],
                    bytes[9],
                    bytes[10],
                    bytes[11],
                    bytes[12],
                    bytes[13],
                    bytes[14],
                    bytes[15]
                ).into()))
            }
            Native::NodeCryptoRandomBytes => {
                let Some(size_val) = arguments.first() else {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "The \"size\" argument must be of type number".into(),
                    });
                };
                let size_num = match size_val {
                    Value::Number(n) => *n,
                    _ => {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: "The \"size\" argument must be of type number".into(),
                        });
                    }
                };
                if size_num.is_nan() || size_num < 0.0 || size_num > 2147483647.0 {
                    return Err(ExecutionError::RangeErrorMessage {
                        message: "The value of \"size\" is out of range".into(),
                    });
                }
                let size = size_num as usize;
                let bytes = self.random_bytes(size, instruction)?;
                let result = node_bytes_value(bytes);
                if let Some(cb) = arguments.get(1)
                    && !matches!(cb, Value::Undefined)
                {
                    if !is_callable(cb) {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: "The \"cb\" argument must be of type function".into(),
                        });
                    }
                    self.call_value(cb, Value::Undefined, vec![Value::Null, result], instruction)?;
                    return Ok(Value::Undefined);
                }
                Ok(result)
            }
            Native::NodeCryptoRandomFillSync => {
                let Some(buffer_val) = arguments.first() else {
                    return Err(ExecutionError::TypeErrorMessage {
                        message:
                            "The \"buffer\" argument must be an ArrayBufferView or ArrayBuffer"
                                .into(),
                    });
                };
                match buffer_val {
                    Value::TypedArray(view) => {
                        let total_len = view.len() * view.kind.width();
                        let offset = arguments.get(1).map(|v| v.as_number()).unwrap_or(0.0);
                        if offset.is_nan() || offset < 0.0 || offset > total_len as f64 {
                            return Err(ExecutionError::RangeErrorMessage {
                                message: "The value of \"offset\" is out of range".into(),
                            });
                        }
                        let offset = offset as usize;
                        let size = arguments
                            .get(2)
                            .map(|v| v.as_number())
                            .unwrap_or((total_len - offset) as f64);
                        if size.is_nan() || size < 0.0 || offset + (size as usize) > total_len {
                            return Err(ExecutionError::RangeErrorMessage {
                                message: "The value of \"size\" is out of range".into(),
                            });
                        }
                        let size = size as usize;
                        if size > 0 {
                            let random = self.random_bytes(size, instruction)?;
                            view.buffer.bytes.borrow_mut()
                                [view.offset + offset..view.offset + offset + size]
                                .copy_from_slice(&random);
                        }
                        Ok(buffer_val.clone())
                    }
                    Value::ArrayBuffer(buffer) => {
                        let total_len = buffer.bytes.borrow().len();
                        let offset = arguments.get(1).map(|v| v.as_number()).unwrap_or(0.0);
                        if offset.is_nan() || offset < 0.0 || offset > total_len as f64 {
                            return Err(ExecutionError::RangeErrorMessage {
                                message: "The value of \"offset\" is out of range".into(),
                            });
                        }
                        let offset = offset as usize;
                        let size = arguments
                            .get(2)
                            .map(|v| v.as_number())
                            .unwrap_or((total_len - offset) as f64);
                        if size.is_nan() || size < 0.0 || offset + (size as usize) > total_len {
                            return Err(ExecutionError::RangeErrorMessage {
                                message: "The value of \"size\" is out of range".into(),
                            });
                        }
                        let size = size as usize;
                        if size > 0 {
                            let random = self.random_bytes(size, instruction)?;
                            buffer.bytes.borrow_mut()[offset..offset + size]
                                .copy_from_slice(&random);
                        }
                        Ok(buffer_val.clone())
                    }
                    Value::DataView(view) => {
                        let total_len = view.byte_length().unwrap_or(0);
                        let offset = arguments.get(1).map(|v| v.as_number()).unwrap_or(0.0);
                        if offset.is_nan() || offset < 0.0 || offset > total_len as f64 {
                            return Err(ExecutionError::RangeErrorMessage {
                                message: "The value of \"offset\" is out of range".into(),
                            });
                        }
                        let offset = offset as usize;
                        let size = arguments
                            .get(2)
                            .map(|v| v.as_number())
                            .unwrap_or((total_len - offset) as f64);
                        if size.is_nan() || size < 0.0 || offset + (size as usize) > total_len {
                            return Err(ExecutionError::RangeErrorMessage {
                                message: "The value of \"size\" is out of range".into(),
                            });
                        }
                        let size = size as usize;
                        if size > 0 {
                            let random = self.random_bytes(size, instruction)?;
                            view.buffer.bytes.borrow_mut()
                                [view.offset + offset..view.offset + offset + size]
                                .copy_from_slice(&random);
                        }
                        Ok(buffer_val.clone())
                    }
                    _ => Err(ExecutionError::TypeErrorMessage {
                        message:
                            "The \"buffer\" argument must be an ArrayBufferView or ArrayBuffer"
                                .into(),
                    }),
                }
            }
            Native::NodeCryptoGetHashes => Ok(Value::Array(ArrayData::new(vec![
                Value::String("md5".into()),
                Value::String("sha1".into()),
                Value::String("sha256".into()),
                Value::String("sha512".into()),
            ]))),
            Native::NodeCryptoGetRandomValues => {
                let Some(arg) = arguments.first() else {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "The \"typedArray\" argument must be an instance of TypedArray"
                            .into(),
                    });
                };
                let Value::TypedArray(view) = arg else {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "The \"typedArray\" argument must be an instance of TypedArray"
                            .into(),
                    });
                };
                if matches!(
                    view.kind,
                    TypedKind::Float16 | TypedKind::Float32 | TypedKind::Float64
                ) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "The \"typedArray\" argument must be an integer-based TypedArray"
                            .into(),
                    });
                }
                let total_len = view.len() * view.kind.width();
                if total_len > 65536 {
                    return Err(ExecutionError::RangeErrorMessage {
                        message: "The ArrayBufferView's byte length exceeds the allowed limit of 65536 bytes".into(),
                    });
                }
                if total_len > 0 {
                    let random = self.random_bytes(total_len, instruction)?;
                    view.buffer.bytes.borrow_mut()[view.offset..view.offset + total_len]
                        .copy_from_slice(&random);
                }
                Ok(arg.clone())
            }
            Native::NodeCryptoTimingSafeEqual => {
                let Some(arg_a) = arguments.first() else {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "The \"a\" and \"b\" arguments must be of type ArrayBufferView or ArrayBuffer".into(),
                    });
                };
                let Some(arg_b) = arguments.get(1) else {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "The \"a\" and \"b\" arguments must be of type ArrayBufferView or ArrayBuffer".into(),
                    });
                };
                let bytes_a = view_or_buffer_bytes(arg_a)?;
                let bytes_b = view_or_buffer_bytes(arg_b)?;
                if bytes_a.len() != bytes_b.len() {
                    return Err(ExecutionError::RangeErrorMessage {
                        message: "Input buffers must have the same byte length".into(),
                    });
                }
                let mut diff = 0_u8;
                for (&x, &y) in bytes_a.iter().zip(bytes_b.iter()) {
                    diff |= x ^ y;
                }
                Ok(Value::Boolean(std::hint::black_box(diff) == 0))
            }
            Native::NodeCryptoCreateHash => node_hash(arguments),
            Native::NodeCryptoCreateHmac => node_hmac(arguments),
            Native::NodeHashUpdate => {
                let state = node_hash_state(receiver)?;
                let mut state = state.borrow_mut();
                if state.digested {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Digest already called".into(),
                    });
                }
                state.bytes.extend(node_body_bytes(arguments.first())?);
                Ok(receiver.clone())
            }
            Native::NodeHashDigest => {
                let state = node_hash_state(receiver)?;
                let mut state = state.borrow_mut();
                if state.digested {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Digest already called".into(),
                    });
                }
                state.digested = true;
                let raw_digest = if let Some(key) = &state.hmac_key {
                    node_hmac_digest(state.algorithm, key, &state.bytes)
                } else {
                    node_hash_digest(state.algorithm, &state.bytes)
                };
                let encoding = arguments.first().map(Value::to_display_string);
                match encoding.as_deref() {
                    None => Ok(node_bytes_value(raw_digest)),
                    Some("hex") => Ok(Value::String(crate::to_hex_text(&raw_digest).into())),
                    Some("base64") => Ok(Value::String(crate::to_base64_text(
                        &raw_digest,
                        false,
                        false,
                    ).into())),
                    Some("base64url") => Ok(Value::String(crate::to_base64_text(
                        &raw_digest,
                        true,
                        true,
                    ).into())),
                    Some(encoding) => Err(ExecutionError::TypeErrorMessage {
                        message: format!("Unknown encoding: {encoding}"),
                    }),
                }
            }
            _ => Err(ExecutionError::TypeErrorMessage {
                message: "Invalid native crypto operation".into(),
            }),
        }
    }
}
