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
invalid!(
    kiln_temporal_from_fields(
        kind: u32,
        numbers: *const f64,
        number_count: usize,
        texts: *const u8,
        text_offsets: *const i64,
        text_count: usize,
        option_texts: *const u8,
        option_offsets: *const i64,
        option_count: usize,
        out: *mut *mut c_void,
    )
);
invalid!(
    kiln_temporal_construct(
        kind: u32,
        numbers: *const f64,
        number_count: usize,
        texts: *const u8,
        text_offsets: *const i64,
        text_count: usize,
        now_high: i64,
        now_low: u64,
        out: *mut *mut c_void,
    )
);
invalid!(
    kiln_temporal_duration_from_fields(numbers: *const f64, count: usize, out: *mut *mut c_void)
);
invalid!(
    kiln_temporal_parse(
        kind: u32,
        bytes: *const u8,
        len: usize,
        option_texts: *const u8,
        option_offsets: *const i64,
        option_count: usize,
        out: *mut *mut c_void,
    )
);
invalid!(
    kiln_temporal_now(
        kind: u32,
        high: i64,
        low: u64,
        zone: *const u8,
        zone_len: usize,
        out: *mut *mut c_void,
    )
);
invalid!(kiln_temporal_instant_from_epoch_milliseconds(value: f64, out: *mut *mut c_void));
invalid!(kiln_temporal_instant_from_epoch_nanoseconds(high: i64, low: u64, out: *mut *mut c_void));
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_free(_handle: *mut c_void) {}
invalid!(kiln_temporal_kind(handle: *const c_void));
invalid!(
    kiln_temporal_get(
        handle: *const c_void,
        name: *const u8,
        name_len: usize,
        out_number: *mut f64,
        out_text: *mut u8,
        capacity: usize,
        out_shape: *mut i32,
    )
);
invalid!(kiln_temporal_to_string(handle: *const c_void, buffer: *mut u8, capacity: usize));
invalid!(
    kiln_temporal_compare(
        left: *const c_void,
        right: *const c_void,
        relative_to: *const c_void,
        option_texts: *const u8,
        option_offsets: *const i64,
        option_count: usize,
        out: *mut i32,
    )
);
invalid!(kiln_temporal_invoke(call: *const c_void));
invalid!(kiln_temporal_to_kind(handle: *const c_void, kind: u32, out: *mut *mut c_void));
invalid!(kiln_temporal_last_error(out_kind: *mut i32, buffer: *mut u8, capacity: usize));
