//! Stateless WebCrypto algorithm operations over borrowed byte spans. The JS facade owns key
//! branding and lifetime; this seam owns only the reviewed cryptographic operations.

use core::slice;

const INVALID_ARGUMENT: i32 = -1;
const INVALID_KEY: i32 = -2;
const INVALID_LENGTH: i32 = -3;
const BUFFER_TOO_SMALL: i32 = -4;
const AUTHENTICATION_FAILED: i32 = -5;
const UNSUPPORTED: i32 = -6;
const RANDOM_UNAVAILABLE: i32 = -7;

unsafe fn borrowed<'a>(pointer: *const u8, length: usize) -> Option<&'a [u8]> {
    if length > isize::MAX as usize || (length != 0 && pointer.is_null()) {
        return None;
    }
    if length == 0 {
        return Some(&[]);
    }
    // SAFETY: the caller's ABI contract keeps each non-null input readable for this call.
    Some(unsafe { slice::from_raw_parts(pointer, length) })
}

fn error_code(error: kiln_webcrypto::Error) -> i32 {
    match error {
        kiln_webcrypto::Error::InvalidLength => INVALID_LENGTH,
        kiln_webcrypto::Error::InvalidKey => INVALID_KEY,
        kiln_webcrypto::Error::AuthenticationFailed => AUTHENTICATION_FAILED,
        kiln_webcrypto::Error::RandomUnavailable => RANDOM_UNAVAILABLE,
        kiln_webcrypto::Error::UnsupportedOperation => UNSUPPORTED,
    }
}

/// Execute one shared RustCrypto WebCrypto operation.
///
/// Operation numbers and byte-span ordering are declared by `kiln-webcrypto::operation` and the
/// shared TypeScript bootstrap. The return-size query is pure except for key generation; callers
/// allocate with a fixed generation bound instead of querying that operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_webcrypto_execute(
    op: u32,
    key: *const u8,
    key_length: usize,
    data: *const u8,
    data_length: usize,
    iv: *const u8,
    iv_length: usize,
    additional_data: *const u8,
    additional_data_length: usize,
    output_length: usize,
    output: *mut u8,
    output_capacity: usize,
    required: *mut usize,
) -> i32 {
    if required.is_null()
        || output_capacity > isize::MAX as usize
        || (output_capacity != 0 && output.is_null())
    {
        return INVALID_ARGUMENT;
    }
    // SAFETY: each pointer/length pair is validated by `borrowed`; the caller grants these spans
    // for the duration of this synchronous call.
    let Some(key) = (unsafe { borrowed(key, key_length) }) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: as above.
    let Some(data) = (unsafe { borrowed(data, data_length) }) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: as above.
    let Some(iv) = (unsafe { borrowed(iv, iv_length) }) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: as above.
    let Some(additional_data) = (unsafe { borrowed(additional_data, additional_data_length) })
    else {
        return INVALID_ARGUMENT;
    };
    let result = match kiln_webcrypto::execute(op, key, data, iv, additional_data, output_length) {
        Ok(result) => result,
        Err(error) => return error_code(error),
    };
    // SAFETY: `required` is non-null and writable per the ABI caller's contract.
    unsafe { required.write(result.len()) };
    if output_capacity < result.len() {
        return BUFFER_TOO_SMALL;
    }
    if result.is_empty() {
        return 0;
    }
    if output.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: caller provides a writable span of at least `output_capacity` bytes, checked above.
    unsafe { core::ptr::copy_nonoverlapping(result.as_ptr(), output, result.len()) };
    0
}
