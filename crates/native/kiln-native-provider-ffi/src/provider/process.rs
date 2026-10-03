//! Synchronous child execution for the shared `node:child_process` facade.
//!
//! This is a host capability, not a second JavaScript surface: the TypeScript builtin validates
//! Node options and turns the captured byte streams into Buffer/string results.

use super::{KilnString, validate_string};
use core::ffi::c_void;
use std::process::Command;

const INVALID_ARGUMENT: i32 = -1;

pub struct ProcessOutput {
    status: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    error: Option<String>,
}

fn read_argument(view: *const KilnString) -> Option<String> {
    // SAFETY: `view` is borrowed from the C caller for this synchronous call.
    let value = unsafe { validate_string(view) }?;
    let units = kiln_string::StringUnits::from_view(value);
    let text = units.to_scalar_string().ok()?;
    if text.as_bytes().contains(&0) {
        return None;
    }
    Some(text)
}

/// Execute one command and return an opaque owner for its complete result.
///
/// # Safety
/// `file` names one readable `KilnString`; when `argc != 0`, `argv` names `argc` readable
/// `KilnString` views. `out` is writable. All spans remain valid for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_process_exec_file_sync(
    file: *const KilnString,
    argv: *const KilnString,
    argc: usize,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() || argc > isize::MAX as usize / core::mem::size_of::<KilnString>() {
        return INVALID_ARGUMENT;
    }
    if argc != 0 && argv.is_null() {
        return INVALID_ARGUMENT;
    }
    let Some(file) = read_argument(file) else {
        return INVALID_ARGUMENT;
    };
    let mut args = Vec::new();
    if args.try_reserve_exact(argc).is_err() {
        return INVALID_ARGUMENT;
    }
    for index in 0..argc {
        // SAFETY: the caller provides `argc` consecutive borrowed views.
        let Some(value) = read_argument(unsafe { argv.add(index) }) else {
            return INVALID_ARGUMENT;
        };
        args.push(value);
    }

    let result = match Command::new(file).args(args).output() {
        Ok(output) => ProcessOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
            error: None,
        },
        Err(error) => ProcessOutput {
            status: -1,
            stdout: Vec::new(),
            stderr: Vec::new(),
            error: Some(error.to_string()),
        },
    };
    // SAFETY: `out` is writable per the function contract. Ownership transfers to the caller.
    unsafe { out.write(Box::into_raw(Box::new(result)).cast()) };
    0
}

/// Borrow the stdout, stderr and status fields of a completed command.
///
/// # Safety
/// `result` must be an owner returned by [`kiln_process_exec_file_sync`]. Every out pointer must
/// be writable. Returned byte spans remain valid until [`kiln_process_output_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_process_output_view(
    result: *const c_void,
    stdout: *mut *const u8,
    stdout_len: *mut usize,
    stderr: *mut *const u8,
    stderr_len: *mut usize,
    status: *mut i32,
    error: *mut *const u8,
    error_len: *mut usize,
) -> i32 {
    if result.is_null()
        || stdout.is_null()
        || stdout_len.is_null()
        || stderr.is_null()
        || stderr_len.is_null()
        || status.is_null()
        || error.is_null()
        || error_len.is_null()
    {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the opaque pointer was created by this module and is alive until its free call.
    let result = unsafe { &*result.cast::<ProcessOutput>() };
    unsafe {
        stdout.write(result.stdout.as_ptr());
        stdout_len.write(result.stdout.len());
        stderr.write(result.stderr.as_ptr());
        stderr_len.write(result.stderr.len());
        status.write(result.status);
    }
    if let Some(message) = result.error.as_deref() {
        unsafe {
            error.write(message.as_ptr());
            error_len.write(message.len());
        }
    } else {
        unsafe {
            error.write(core::ptr::null());
            error_len.write(0);
        }
    }
    0
}

/// Release the output owner returned by [`kiln_process_exec_file_sync`].
///
/// # Safety
/// `result` must be null or a live owner returned by this module, and may be released once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_process_output_free(result: *mut c_void) {
    if !result.is_null() {
        // SAFETY: ownership was transferred by `Box::into_raw` and may be reclaimed exactly once.
        drop(unsafe { Box::from_raw(result.cast::<ProcessOutput>()) });
    }
}
