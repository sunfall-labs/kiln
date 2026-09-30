use super::INVALID_ARGUMENT;
use core::ffi::c_void;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_regexp_compile(
    _pattern: *const super::KilnString,
    _flags: *const super::KilnString,
    _out: *mut *mut c_void,
) -> i32 {
    INVALID_ARGUMENT
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_regexp_free(_handle: *mut c_void) {}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_regexp_search(
    _handle: *mut c_void,
    _subject: *const super::KilnString,
    _start: usize,
    _ranges: *mut i64,
    _capacity: usize,
) -> i32 {
    INVALID_ARGUMENT
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_regexp_group_count(_handle: *const c_void) -> usize {
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_regexp_group_name(
    _handle: *const c_void,
    _index: usize,
    _buffer: *mut u8,
    _capacity: usize,
) -> i32 {
    INVALID_ARGUMENT
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_regexp_named_group_count(_handle: *const c_void) -> usize {
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_regexp_named_group_at(
    _handle: *const c_void,
    _index: usize,
    _buffer: *mut u8,
    _capacity: usize,
    _from: *mut i64,
    _to: *mut i64,
) -> i32 {
    INVALID_ARGUMENT
}
