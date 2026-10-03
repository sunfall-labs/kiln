use core::{ptr, slice};
use kiln_compression::{Codec, Direction, Format};

const INVALID: i32 = -1;
const OUTPUT_ERROR: i32 = -2;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_compression_state_create(format: u32, direction: u32) -> *mut Codec {
    let format = match format {
        1 => Format::Gzip,
        2 => Format::Deflate,
        3 => Format::DeflateRaw,
        _ => return ptr::null_mut(),
    };
    let direction = match direction {
        1 => Direction::Compress,
        2 => Direction::Decompress,
        _ => return ptr::null_mut(),
    };
    Box::into_raw(Box::new(Codec::new(format, direction)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_compression_state_free(state: *mut Codec) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state) });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_compression_state_push(
    state: *mut Codec,
    input: *const u8,
    input_length: usize,
    finish: u32,
    output: *mut *mut u8,
    output_length: *mut usize,
) -> i32 {
    if state.is_null()
        || output.is_null()
        || output_length.is_null()
        || (input_length != 0 && input.is_null())
        || input_length > isize::MAX as usize
    {
        return INVALID;
    }
    let input = if input_length == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(input, input_length) }
    };
    let result = match unsafe { &mut *state }.push(input, finish != 0) {
        Ok(value) => value,
        Err(_) => return OUTPUT_ERROR,
    };
    let mut boxed = result.into_boxed_slice();
    unsafe {
        output_length.write(boxed.len());
        output.write(boxed.as_mut_ptr());
    }
    core::mem::forget(boxed);
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_compression_output_free(output: *mut u8, length: usize) {
    if !output.is_null() {
        drop(unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(output, length)) });
    }
}
