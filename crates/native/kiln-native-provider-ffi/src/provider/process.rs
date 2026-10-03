//! Synchronous child execution for the shared `node:child_process` facade.
//!
//! This is a host capability, not a second JavaScript surface: the TypeScript builtin validates
//! Node options and turns the captured byte streams into Buffer/string results.

use super::{KilnString, validate_string};
use core::ffi::c_void;
use std::process::{Command, ExitStatus, Stdio};

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

const INVALID_ARGUMENT: i32 = -1;

pub struct ProcessOutput {
    pid: u32,
    status: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    error: Option<String>,
    signal: Option<String>,
}

fn signal_name(status: ExitStatus) -> Option<String> {
    #[cfg(unix)]
    {
        let signal = status.signal()?;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let name = match signal {
            1 => "SIGHUP",
            2 => "SIGINT",
            3 => "SIGQUIT",
            4 => "SIGILL",
            5 => "SIGTRAP",
            6 => "SIGABRT",
            7 => "SIGBUS",
            8 => "SIGFPE",
            9 => "SIGKILL",
            10 => "SIGUSR1",
            11 => "SIGSEGV",
            12 => "SIGUSR2",
            13 => "SIGPIPE",
            14 => "SIGALRM",
            15 => "SIGTERM",
            16 => "SIGSTKFLT",
            17 => "SIGCHLD",
            18 => "SIGCONT",
            19 => "SIGSTOP",
            20 => "SIGTSTP",
            21 => "SIGTTIN",
            22 => "SIGTTOU",
            23 => "SIGURG",
            24 => "SIGXCPU",
            25 => "SIGXFSZ",
            26 => "SIGVTALRM",
            27 => "SIGPROF",
            28 => "SIGWINCH",
            29 => "SIGIO",
            30 => "SIGPWR",
            31 => "SIGSYS",
            _ => return Some(format!("SIG{signal}")),
        };
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        let name = match signal {
            1 => "SIGHUP",
            2 => "SIGINT",
            3 => "SIGQUIT",
            4 => "SIGILL",
            5 => "SIGTRAP",
            6 => "SIGABRT",
            7 => "SIGEMT",
            8 => "SIGFPE",
            9 => "SIGKILL",
            10 => "SIGBUS",
            11 => "SIGSEGV",
            12 => "SIGSYS",
            13 => "SIGPIPE",
            14 => "SIGALRM",
            15 => "SIGTERM",
            16 => "SIGURG",
            17 => "SIGSTOP",
            18 => "SIGTSTP",
            19 => "SIGCONT",
            20 => "SIGCHLD",
            21 => "SIGTTIN",
            22 => "SIGTTOU",
            23 => "SIGIO",
            24 => "SIGXCPU",
            25 => "SIGXFSZ",
            26 => "SIGVTALRM",
            27 => "SIGPROF",
            28 => "SIGWINCH",
            29 => "SIGINFO",
            30 => "SIGUSR1",
            31 => "SIGUSR2",
            _ => return Some(format!("SIG{signal}")),
        };
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        )))]
        let name = return Some(format!("SIG{signal}"));
        Some(name.to_owned())
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
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

    let result = match Command::new(file)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => {
            let pid = child.id();
            match child.wait_with_output() {
                Ok(output) => ProcessOutput {
                    pid,
                    status: output.status.code().unwrap_or(-1),
                    stdout: output.stdout,
                    stderr: output.stderr,
                    error: None,
                    signal: signal_name(output.status),
                },
                Err(error) => ProcessOutput {
                    pid,
                    status: -1,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    error: Some(error.to_string()),
                    signal: None,
                },
            }
        }
        Err(error) => ProcessOutput {
            pid: 0,
            status: -1,
            stdout: Vec::new(),
            stderr: Vec::new(),
            error: Some(error.to_string()),
            signal: None,
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
    pid: *mut u32,
    error: *mut *const u8,
    error_len: *mut usize,
    signal: *mut *const u8,
    signal_len: *mut usize,
) -> i32 {
    if result.is_null()
        || stdout.is_null()
        || stdout_len.is_null()
        || stderr.is_null()
        || stderr_len.is_null()
        || status.is_null()
        || pid.is_null()
        || error.is_null()
        || error_len.is_null()
        || signal.is_null()
        || signal_len.is_null()
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
        pid.write(result.pid);
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
    if let Some(signal_name) = result.signal.as_deref() {
        unsafe {
            signal.write(signal_name.as_ptr());
            signal_len.write(signal_name.len());
        }
    } else {
        unsafe {
            signal.write(core::ptr::null());
            signal_len.write(0);
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
