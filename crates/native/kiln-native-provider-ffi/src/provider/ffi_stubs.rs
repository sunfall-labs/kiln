use super::INVALID_ARGUMENT;
use core::ffi::c_void;

#[repr(C)]
pub struct FfiValue {
    kind: u32,
    reserved: u32,
    bits: u64,
}
type Dispatch = unsafe extern "C" fn(*mut c_void, *const FfiValue, u64, *mut FfiValue) -> i32;
macro_rules! invalid {
        ($name:ident($($arg:ident: $ty:ty),*)) => {
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn $name($($arg: $ty),*) -> i32 {
                $(let _ = $arg;)*
                INVALID_ARGUMENT
            }
        };
    }
invalid!(kiln_ffi_open(path: *const u8, path_len: u64, out_library: *mut u64));
invalid!(kiln_ffi_close(library: u64));
invalid!(
    kiln_ffi_prepare(
        library: u64,
        symbol: *const u8,
        symbol_len: u64,
        argument_types: *const u32,
        argument_count: u64,
        result_type: u32,
        out_function: *mut u64,
    )
);
invalid!(
    kiln_ffi_call(
        function: u64,
        arguments: *const FfiValue,
        argument_count: u64,
        out_result: *mut FfiValue,
    )
);
invalid!(
    kiln_ffi_register_callback(
        argument_types: *const u32,
        argument_count: u64,
        result_type: u32,
        dispatch: Option<Dispatch>,
        context: *mut c_void,
        out_pointer: *mut u64,
    )
);
invalid!(kiln_ffi_unregister_callback(pointer: u64));
invalid!(kiln_ffi_last_error(out_bytes: *mut *const u8, out_len: *mut u64));
