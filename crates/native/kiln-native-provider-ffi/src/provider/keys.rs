//! Opaque, realm-owned native adapters for the shared validated asymmetric-key core.

use core::slice;

use kiln_key_core::{
    ImportedKey, KeyEncoding, KeyFormat, KeyType, import_rsa, import_rsa_any, import_rsa_der,
    import_rsa_jwk_json,
};

use super::realm::{active_crypto_key_insert, active_crypto_key_remove, with_active_crypto_key};

const INVALID_ARGUMENT: i32 = -1;
const INVALID_KEY: i32 = -2;
const UNSUPPORTED: i32 = -3;
const BUFFER_TOO_SMALL: i32 = -4;
const WRONG_KEY_TYPE: i32 = -5;

/// Import PEM, DER, or a JSON RSA JWK and return a realm-scoped opaque handle.
/// `kind` is 1=private PEM, 2=public PEM, 3=private PKCS#1 DER, 4=private PKCS#8 DER,
/// 5=public PKCS#1 DER, 6=public SPKI DER, 7=public/automatic RSA JWK JSON, 8=private JWK JSON,
/// 9=automatic PEM kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_crypto_key_import(
    data: *const u8,
    length: usize,
    kind: u32,
    out_handle: *mut u64,
) -> i32 {
    if out_handle.is_null() || (length != 0 && data.is_null()) || length > isize::MAX as usize {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the ABI caller guarantees the input span is readable for this call; null and the
    // Rust slice length bound were checked above.
    let bytes = if length == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(data, length) }
    };
    let parsed = match kind {
        1 => import_rsa(bytes, KeyEncoding::Pem, KeyType::Private),
        2 => import_rsa(bytes, KeyEncoding::Pem, KeyType::Public),
        3 => import_rsa_der(bytes, KeyType::Private, KeyFormat::Pkcs1),
        4 => import_rsa_der(bytes, KeyType::Private, KeyFormat::Pkcs8),
        5 => import_rsa_der(bytes, KeyType::Public, KeyFormat::Pkcs1),
        6 => import_rsa_der(bytes, KeyType::Public, KeyFormat::Spki),
        7 => import_rsa_jwk_json(bytes),
        8 => match import_rsa_jwk_json(bytes) {
            Ok(key @ ImportedKey::RsaPrivate(_)) => Ok(key),
            Ok(ImportedKey::RsaPublic(_)) => Err(kiln_key_core::KeyError::WrongKeyType),
            Err(error) => Err(error),
        },
        9 => import_rsa_any(bytes, KeyEncoding::Pem),
        _ => return UNSUPPORTED,
    };
    let key = match parsed {
        Ok(key) => key,
        Err(
            kiln_key_core::KeyError::UnsupportedEncoding
            | kiln_key_core::KeyError::UnsupportedKeyType
            | kiln_key_core::KeyError::UnsupportedKeySize,
        ) => {
            return UNSUPPORTED;
        }
        Err(kiln_key_core::KeyError::WrongKeyType) => return WRONG_KEY_TYPE,
        Err(kiln_key_core::KeyError::InvalidKey) => {
            return INVALID_KEY;
        }
    };
    let Some(handle) = active_crypto_key_insert(key) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: the ABI contract requires a writable output pointer; it was checked above.
    unsafe { out_handle.write(handle) };
    0
}

/// Derive a public-only handle from either an RSA public or private key handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_crypto_key_public(handle: u64, out_handle: *mut u64) -> i32 {
    if out_handle.is_null() {
        return INVALID_ARGUMENT;
    }
    let Some(public) = with_active_crypto_key(handle, ImportedKey::public_key) else {
        return INVALID_ARGUMENT;
    };
    let Some(public_handle) = active_crypto_key_insert(public) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: the ABI contract requires a writable output pointer; it was checked above.
    unsafe { out_handle.write(public_handle) };
    0
}

/// Release a realm-owned key handle after its JS `KeyObject` is finalized. Repeated release and
/// delivery after realm disposal are harmless.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_crypto_key_free(handle: u64) -> i32 {
    active_crypto_key_remove(handle);
    0
}

/// Return 1=public / 2=private and the RSA modulus bit length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_crypto_key_info(
    handle: u64,
    out_type: *mut u32,
    out_bits: *mut u32,
) -> i32 {
    if out_type.is_null() || out_bits.is_null() {
        return INVALID_ARGUMENT;
    }
    let Some((key_type, bits)) = with_active_crypto_key(handle, |key| {
        let ty = match key.key_type() {
            KeyType::Public => 1,
            KeyType::Private => 2,
        };
        (ty, key.modulus_bits())
    }) else {
        return INVALID_ARGUMENT;
    };
    let Ok(bits) = u32::try_from(bits) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: the ABI contract requires writable outputs; both pointers were checked above.
    unsafe {
        out_type.write(key_type);
        out_bits.write(bits);
    }
    0
}

/// Export an imported key. If `capacity` is zero this reports the required size. Otherwise a
/// short output buffer returns `BUFFER_TOO_SMALL` and reports the required size without writing.
/// `format` is 1=PKCS#1, 2=PKCS#8, 3=SPKI; `encoding` is 0=DER, 1=PEM.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_crypto_key_export(
    handle: u64,
    format: u32,
    encoding: u32,
    output: *mut u8,
    capacity: usize,
    out_length: *mut usize,
) -> i32 {
    if out_length.is_null() || (capacity != 0 && output.is_null()) {
        return INVALID_ARGUMENT;
    }
    let format = match format {
        1 => KeyFormat::Pkcs1,
        2 => KeyFormat::Pkcs8,
        3 => KeyFormat::Spki,
        4 => KeyFormat::Jwk,
        _ => return UNSUPPORTED,
    };
    let encoding = match encoding {
        0 => KeyEncoding::Der,
        1 => KeyEncoding::Pem,
        2 => KeyEncoding::Json,
        _ => return UNSUPPORTED,
    };
    let Some(bytes) = with_active_crypto_key(handle, |key| key.export(format, encoding)) else {
        return INVALID_ARGUMENT;
    };
    let Ok(bytes) = bytes else {
        return UNSUPPORTED;
    };
    let required = bytes.len();
    // SAFETY: the ABI contract requires a writable length slot; it was checked above.
    unsafe { out_length.write(required) };
    if capacity == 0 {
        return 0;
    }
    if capacity < required {
        return BUFFER_TOO_SMALL;
    }
    // SAFETY: the caller supplied `capacity` writable bytes, and `capacity >= required`.
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), output, required) };
    0
}
