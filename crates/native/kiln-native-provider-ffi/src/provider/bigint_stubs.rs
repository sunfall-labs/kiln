use super::INVALID_ARGUMENT;
use core::ffi::c_void;

macro_rules! invalid {
        ($name:ident($($arg:ident: $ty:ty),*)) => {
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn $name($($arg: $ty),*) -> i32 {
                $(let _ = $arg;)*
                INVALID_ARGUMENT
            }
        };
    }
invalid!(kiln_bigint_parse(text: *const u8, len: usize, radix: u32, out: *mut *mut c_void));
invalid!(kiln_bigint_from_double(value: f64, out: *mut *mut c_void));
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_bigint_free(_handle: *mut c_void) {}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_bigint_to_double(_handle: *const c_void) -> f64 {
    f64::NAN
}
invalid!(kiln_bigint_to_string(handle: *const c_void, radix: u32, buffer: *mut u8, capacity: usize));
invalid!(kiln_bigint_binary(operation: u32, left: *const c_void, right: *const c_void, out: *mut *mut c_void));
invalid!(kiln_bigint_negate(handle: *const c_void, out: *mut *mut c_void));
invalid!(kiln_bigint_compare(left: *const c_void, right: *const c_void));
invalid!(kiln_bigint_is_zero(handle: *const c_void));
invalid!(kiln_bigint_to_i128(handle: *const c_void, out_high: *mut i64, out_low: *mut u64));
