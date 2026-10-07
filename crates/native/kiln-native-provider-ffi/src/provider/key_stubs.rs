//! Link-compatible refusal stubs for a provider package without realm support.

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_crypto_key_import(
    _data: *const u8,
    _length: usize,
    _kind: u32,
    _out_handle: *mut u64,
) -> i32 {
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_crypto_key_public(_handle: u64, _out_handle: *mut u64) -> i32 {
    -1
}

#[unsafe(no_mangle)]
pub extern "C" fn kiln_crypto_key_free(_handle: u64) -> i32 {
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_crypto_key_info(
    _handle: u64,
    _out_type: *mut u32,
    _out_bits: *mut u32,
) -> i32 {
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_crypto_key_export(
    _handle: u64,
    _format: u32,
    _encoding: u32,
    _output: *mut u8,
    _capacity: usize,
    _out_length: *mut usize,
) -> i32 {
    -1
}
