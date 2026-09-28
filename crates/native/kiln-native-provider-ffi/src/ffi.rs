//! Same-thread dynamic-call authority for the native `node:ffi` builtin.
//!
//! The JavaScript builtin validates and normalizes Node's public definition objects. This module
//! owns only the process-local resources that cannot be represented in JavaScript: loader handles,
//! resolved code pointers, and libffi call interfaces. Handles are integers and all state is
//! thread-local, matching the realm and callback execution contract.

use core::ffi::{c_char, c_void};
use libffi::middle::{Arg, Cif, CodePtr, Type, arg};
use libffi::{low, raw};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::rc::Rc;
use std::slice;

use super::{INVALID_ARGUMENT, RUNTIME_ERROR};

pub const TYPE_VOID: u32 = 0;
pub const TYPE_I8: u32 = 1;
pub const TYPE_U8: u32 = 2;
pub const TYPE_I16: u32 = 3;
pub const TYPE_U16: u32 = 4;
pub const TYPE_I32: u32 = 5;
pub const TYPE_U32: u32 = 6;
pub const TYPE_I64: u32 = 7;
pub const TYPE_U64: u32 = 8;
pub const TYPE_F32: u32 = 9;
pub const TYPE_F64: u32 = 10;
pub const TYPE_POINTER: u32 = 11;
pub const TYPE_BOOL: u32 = 12;

/// One normalized argument or result. Integer bits are exact, floats use their IEEE bit pattern,
/// and pointers are represented as `uintptr_t` widened to 64 bits. `kind` is repeated in values so
/// a malformed caller is rejected before libffi observes an incompatible argument layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FfiValue {
    pub kind: u32,
    pub reserved: u32,
    pub bits: u64,
}

union Slot {
    i8_: i8,
    u8_: u8,
    i16_: i16,
    u16_: u16,
    i32_: i32,
    u32_: u32,
    i64_: i64,
    u64_: u64,
    f32_: f32,
    f64_: f64,
    pointer: *mut c_void,
}

impl Slot {
    fn from_value(value: FfiValue) -> Result<Self, String> {
        Ok(match value.kind {
            TYPE_I8 => Self {
                i8_: value.bits as i8,
            },
            TYPE_U8 | TYPE_BOOL => Self {
                u8_: value.bits as u8,
            },
            TYPE_I16 => Self {
                i16_: value.bits as i16,
            },
            TYPE_U16 => Self {
                u16_: value.bits as u16,
            },
            TYPE_I32 => Self {
                i32_: value.bits as i32,
            },
            TYPE_U32 => Self {
                u32_: value.bits as u32,
            },
            TYPE_I64 => Self {
                i64_: value.bits as i64,
            },
            TYPE_U64 => Self { u64_: value.bits },
            TYPE_F32 => Self {
                f32_: f32::from_bits(value.bits as u32),
            },
            TYPE_F64 => Self {
                f64_: f64::from_bits(value.bits),
            },
            TYPE_POINTER => Self {
                pointer: value.bits as usize as *mut c_void,
            },
            other => return Err(format!("unsupported node:ffi value kind {other}")),
        })
    }

    unsafe fn as_arg(&self, kind: u32) -> Arg<'_> {
        // SAFETY: the selected union field is the one initialized by `from_value` for `kind`.
        unsafe {
            match kind {
                TYPE_I8 => arg(&self.i8_),
                TYPE_U8 | TYPE_BOOL => arg(&self.u8_),
                TYPE_I16 => arg(&self.i16_),
                TYPE_U16 => arg(&self.u16_),
                TYPE_I32 => arg(&self.i32_),
                TYPE_U32 => arg(&self.u32_),
                TYPE_I64 => arg(&self.i64_),
                TYPE_U64 => arg(&self.u64_),
                TYPE_F32 => arg(&self.f32_),
                TYPE_F64 => arg(&self.f64_),
                TYPE_POINTER => arg(&self.pointer),
                _ => unreachable!("validated FFI type"),
            }
        }
    }
}

fn ffi_type(kind: u32, argument: bool) -> Result<Type, String> {
    Ok(match kind {
        TYPE_VOID if !argument => Type::void(),
        TYPE_I8 => Type::i8(),
        TYPE_U8 | TYPE_BOOL => Type::u8(),
        TYPE_I16 => Type::i16(),
        TYPE_U16 => Type::u16(),
        TYPE_I32 => Type::i32(),
        TYPE_U32 => Type::u32(),
        TYPE_I64 => Type::i64(),
        TYPE_U64 => Type::u64(),
        TYPE_F32 => Type::f32(),
        TYPE_F64 => Type::f64(),
        TYPE_POINTER => Type::pointer(),
        TYPE_VOID => return Err("void is not a valid node:ffi argument type".to_owned()),
        other => return Err(format!("unsupported node:ffi type {other}")),
    })
}

struct Library {
    pointer: *mut c_void,
}

impl Drop for Library {
    fn drop(&mut self) {
        if !self.pointer.is_null() {
            // SAFETY: `pointer` is an owned successful `dlopen` result and is dropped once.
            unsafe { libc::dlclose(self.pointer) };
        }
    }
}

struct Function {
    library: u64,
    pointer: *mut c_void,
    cif: Cif,
    arguments: Vec<u32>,
    result: u32,
}

type Dispatch = unsafe extern "C" fn(
    context: *mut c_void,
    arguments: *const FfiValue,
    argument_count: u64,
    result: *mut FfiValue,
) -> i32;

struct CallbackData {
    dispatch: Dispatch,
    context: *mut c_void,
    arguments: Vec<u32>,
    result: u32,
    active: Cell<u32>,
}

struct Callback {
    cif: Box<Cif>,
    allocation: *mut low::ffi_closure,
    data: Box<CallbackData>,
}

impl Drop for Callback {
    fn drop(&mut self) {
        // The allocation borrows both owners until it is freed here.
        let _ = (&self.cif, &self.data);
        // SAFETY: this non-null allocation came from `try_closure_alloc` and is freed once.
        unsafe { low::closure_free(self.allocation) };
    }
}

/// One realm's `node:ffi` resources (ADR 0070, realms issue 04 step 5): its open libraries,
/// prepared functions and callback closures. It lives in the realm (`NativeRealm::ffi`) and is
/// dropped with it, so a realm's teardown closes what it opened and frees the closures whose
/// userdata points into its heap; another realm on the same thread sees none of it. Handles are
/// numbered per realm.
#[derive(Default)]
pub(crate) struct State {
    next: u64,
    libraries: HashMap<u64, Library>,
    functions: HashMap<u64, Rc<Function>>,
    callbacks: HashMap<u64, Callback>,
    active_libraries: HashMap<u64, u32>,
}

impl State {
    fn handle(&mut self) -> Result<u64, String> {
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| "node:ffi handle space exhausted".to_owned())?;
        Ok(self.next)
    }
}

thread_local! {
    /// The state of calls made outside any realm: only this crate's own tests make them.
    static UNREALMED: RefCell<State> = RefCell::new(State::default());
    /// The last failure's text, an out-parameter of the call that just failed on this thread.
    static LAST_ERROR: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Run `operation` on the active realm's `node:ffi` state.
fn with_state<R>(operation: impl FnOnce(&mut State) -> R) -> R {
    #[cfg(feature = "realm")]
    {
        let mut operation = Some(operation);
        if let Some(result) = crate::realm::with_active_ffi_state(|state| {
            let operation = operation.take().expect("run once");
            operation(&mut state.borrow_mut())
        }) {
            return result;
        }
        let operation = operation.take().expect("not yet run");
        return UNREALMED.with_borrow_mut(operation);
    }
    #[cfg(not(feature = "realm"))]
    UNREALMED.with_borrow_mut(operation)
}

struct ActiveLibrary(u64);

impl Drop for ActiveLibrary {
    fn drop(&mut self) {
        with_state(|state| {
            let remove = if let Some(count) = state.active_libraries.get_mut(&self.0) {
                *count = count.saturating_sub(1);
                *count == 0
            } else {
                false
            };
            if remove {
                state.active_libraries.remove(&self.0);
            }
        });
    }
}

fn clear_error() {
    LAST_ERROR.with_borrow_mut(Vec::clear);
}

fn fail(message: String) -> i32 {
    LAST_ERROR.with_borrow_mut(|stored| {
        stored.clear();
        stored.extend_from_slice(message.as_bytes());
    });
    RUNTIME_ERROR
}

fn guarded(operation: impl FnOnce() -> Result<(), String>) -> i32 {
    clear_error();
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(())) => 0,
        Ok(Err(message)) => fail(message),
        Err(_) => fail("node:ffi provider panicked".to_owned()),
    }
}

unsafe fn bytes<'a>(pointer: *const u8, length: u64, label: &str) -> Result<&'a [u8], String> {
    let length = usize::try_from(length).map_err(|_| format!("{label} is too long"))?;
    if length == 0 {
        return Ok(&[]);
    }
    if pointer.is_null() {
        return Err(format!("{label} pointer is null"));
    }
    // SAFETY: the caller promises `length` readable bytes for the duration of this call.
    Ok(unsafe { slice::from_raw_parts(pointer, length) })
}

unsafe fn values<'a>(pointer: *const FfiValue, count: u64) -> Result<&'a [FfiValue], String> {
    let count = usize::try_from(count).map_err(|_| "node:ffi argument count is too large")?;
    if count == 0 {
        return Ok(&[]);
    }
    if pointer.is_null() {
        return Err("node:ffi argument pointer is null".to_owned());
    }
    // SAFETY: the caller promises `count` readable values for the duration of this call.
    Ok(unsafe { slice::from_raw_parts(pointer, count) })
}

unsafe fn type_codes<'a>(pointer: *const u32, count: u64) -> Result<&'a [u32], String> {
    let count = usize::try_from(count).map_err(|_| "node:ffi type count is too large")?;
    if count == 0 {
        return Ok(&[]);
    }
    if pointer.is_null() {
        return Err("node:ffi type pointer is null".to_owned());
    }
    // SAFETY: the caller promises `count` readable type codes for the duration of this call.
    Ok(unsafe { slice::from_raw_parts(pointer, count) })
}

fn loader_error(prefix: &str) -> String {
    // SAFETY: `dlerror` returns either null or a process-owned NUL-terminated diagnostic.
    let detail = unsafe {
        let pointer = libc::dlerror();
        if pointer.is_null() {
            "unknown dynamic-loader error".to_owned()
        } else {
            CStr::from_ptr(pointer.cast::<c_char>())
                .to_string_lossy()
                .into_owned()
        }
    };
    format!("{prefix}: {detail}")
}

/// Open one library in the current process and return a thread-local owner handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_ffi_open(
    path: *const u8,
    path_len: u64,
    out_library: *mut u64,
) -> i32 {
    guarded(|| {
        if out_library.is_null() {
            return Err("node:ffi library output pointer is null".to_owned());
        }
        // SAFETY: validated by `bytes` against the C ABI contract.
        let path = unsafe { bytes(path, path_len, "node:ffi library path") }?;
        let path =
            CString::new(path).map_err(|_| "node:ffi library path contains NUL".to_owned())?;
        // Clear a prior loader diagnostic before calling `dlopen`.
        // SAFETY: both loader calls follow their platform contracts.
        unsafe { libc::dlerror() };
        let pointer = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if pointer.is_null() {
            return Err(loader_error("node:ffi could not open library"));
        }
        with_state(|state| {
            let handle = state.handle()?;
            state.libraries.insert(handle, Library { pointer });
            // SAFETY: checked non-null and owned by the caller.
            unsafe { out_library.write(handle) };
            Ok(())
        })
    })
}

/// Close a library after invalidating all prepared functions that came from it.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_ffi_close(library: u64) -> i32 {
    guarded(|| {
        with_state(|state| {
            if !state.libraries.contains_key(&library) {
                return Err(format!("node:ffi library handle {library} is not open"));
            }
            if state.active_libraries.get(&library).copied().unwrap_or(0) != 0 {
                return Err(format!(
                    "node:ffi library handle {library} cannot close during a call"
                ));
            }
            state
                .functions
                .retain(|_, function| function.library != library);
            state.libraries.remove(&library);
            Ok(())
        })
    })
}

/// Resolve a symbol and prepare its fixed C signature.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_ffi_prepare(
    library: u64,
    symbol: *const u8,
    symbol_len: u64,
    argument_types: *const u32,
    argument_count: u64,
    result_type: u32,
    out_function: *mut u64,
) -> i32 {
    guarded(|| {
        if out_function.is_null() {
            return Err("node:ffi function output pointer is null".to_owned());
        }
        // SAFETY: validated by the two slice helpers against the C ABI contract.
        let symbol = unsafe { bytes(symbol, symbol_len, "node:ffi symbol") }?;
        let argument_codes = unsafe { type_codes(argument_types, argument_count) }?.to_vec();
        let symbol = CString::new(symbol).map_err(|_| "node:ffi symbol contains NUL".to_owned())?;
        let argument_ffi_types = argument_codes
            .iter()
            .map(|kind| ffi_type(*kind, true))
            .collect::<Result<Vec<_>, _>>()?;
        let result_ffi_type = ffi_type(result_type, false)?;
        with_state(|state| {
            let library_pointer = state
                .libraries
                .get(&library)
                .ok_or_else(|| format!("node:ffi library handle {library} is not open"))?
                .pointer;
            // SAFETY: the library stays open in `state`; `dlsym` borrows the NUL-terminated name.
            unsafe { libc::dlerror() };
            let pointer = unsafe { libc::dlsym(library_pointer, symbol.as_ptr()) };
            if pointer.is_null() {
                return Err(loader_error("node:ffi could not resolve symbol"));
            }
            let cif = Cif::new(argument_ffi_types, result_ffi_type);
            let handle = state.handle()?;
            state.functions.insert(
                handle,
                Rc::new(Function {
                    library,
                    pointer,
                    cif,
                    arguments: argument_codes,
                    result: result_type,
                }),
            );
            // SAFETY: checked non-null and owned by the caller.
            unsafe { out_function.write(handle) };
            Ok(())
        })
    })
}

fn call(function: &Function, arguments: &[FfiValue]) -> Result<FfiValue, String> {
    if arguments.len() != function.arguments.len() {
        return Err(format!(
            "node:ffi function expects {} arguments, received {}",
            function.arguments.len(),
            arguments.len()
        ));
    }
    for (index, (value, expected)) in arguments.iter().zip(&function.arguments).enumerate() {
        if value.kind != *expected || value.reserved != 0 {
            return Err(format!(
                "node:ffi argument {index} has kind {}, expected {expected}",
                value.kind
            ));
        }
    }
    let slots = arguments
        .iter()
        .copied()
        .map(Slot::from_value)
        .collect::<Result<Vec<_>, _>>()?;
    let ffi_arguments = slots
        .iter()
        .zip(&function.arguments)
        // SAFETY: each slot was initialized from and is selected by this same kind.
        .map(|(slot, kind)| unsafe { slot.as_arg(*kind) })
        .collect::<Vec<_>>();
    let pointer = CodePtr(function.pointer);
    let bits = unsafe {
        // SAFETY: `prepare` constructed this CIF from the same result and argument kinds, and the
        // library remains open while the function is stored.
        match function.result {
            TYPE_VOID => {
                function.cif.call::<()>(pointer, &ffi_arguments);
                0
            }
            TYPE_I8 => function.cif.call::<i8>(pointer, &ffi_arguments) as u64,
            TYPE_U8 | TYPE_BOOL => function.cif.call::<u8>(pointer, &ffi_arguments) as u64,
            TYPE_I16 => function.cif.call::<i16>(pointer, &ffi_arguments) as u64,
            TYPE_U16 => function.cif.call::<u16>(pointer, &ffi_arguments) as u64,
            TYPE_I32 => function.cif.call::<i32>(pointer, &ffi_arguments) as u64,
            TYPE_U32 => function.cif.call::<u32>(pointer, &ffi_arguments) as u64,
            TYPE_I64 => function.cif.call::<i64>(pointer, &ffi_arguments) as u64,
            TYPE_U64 => function.cif.call::<u64>(pointer, &ffi_arguments),
            TYPE_F32 => u64::from(function.cif.call::<f32>(pointer, &ffi_arguments).to_bits()),
            TYPE_F64 => function.cif.call::<f64>(pointer, &ffi_arguments).to_bits(),
            TYPE_POINTER => {
                function.cif.call::<*mut c_void>(pointer, &ffi_arguments) as usize as u64
            }
            _ => return Err("node:ffi function has an invalid result type".to_owned()),
        }
    };
    Ok(FfiValue {
        kind: function.result,
        reserved: 0,
        bits,
    })
}

/// Invoke a prepared function using normalized fixed-width values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_ffi_call(
    function: u64,
    arguments: *const FfiValue,
    argument_count: u64,
    out_result: *mut FfiValue,
) -> i32 {
    guarded(|| {
        if out_result.is_null() {
            return Err("node:ffi result pointer is null".to_owned());
        }
        // SAFETY: validated by `values` against the C ABI contract.
        let arguments = unsafe { values(arguments, argument_count) }?;
        let prepared = with_state(|state| {
            let prepared =
                Rc::clone(state.functions.get(&function).ok_or_else(|| {
                    format!("node:ffi function handle {function} is not prepared")
                })?);
            let count = state.active_libraries.entry(prepared.library).or_insert(0);
            *count = count
                .checked_add(1)
                .ok_or_else(|| "node:ffi active-call count overflow".to_owned())?;
            Ok::<_, String>(prepared)
        })?;
        let _active = ActiveLibrary(prepared.library);
        let _replacement = crate::realm::enter_blocking_scheduler()
            .map_err(|error| format!("node:ffi call: {error}"))?;
        let result = call(&prepared, arguments)?;
        // SAFETY: checked non-null and owned by the caller.
        unsafe { out_result.write(result) };
        Ok(())
    })
}

unsafe fn callback_argument(pointer: *mut c_void, kind: u32) -> Result<FfiValue, String> {
    if pointer.is_null() {
        return Err("node:ffi callback received a null argument slot".to_owned());
    }
    // SAFETY: libffi supplies one naturally aligned slot whose type is the callback CIF's type.
    let bits = unsafe {
        match kind {
            TYPE_I8 => (*pointer.cast::<i8>()) as u64,
            TYPE_U8 | TYPE_BOOL => u64::from(*pointer.cast::<u8>()),
            TYPE_I16 => (*pointer.cast::<i16>()) as u64,
            TYPE_U16 => u64::from(*pointer.cast::<u16>()),
            TYPE_I32 => (*pointer.cast::<i32>()) as u64,
            TYPE_U32 => u64::from(*pointer.cast::<u32>()),
            TYPE_I64 => (*pointer.cast::<i64>()) as u64,
            TYPE_U64 => *pointer.cast::<u64>(),
            TYPE_F32 => u64::from((*pointer.cast::<f32>()).to_bits()),
            TYPE_F64 => (*pointer.cast::<f64>()).to_bits(),
            TYPE_POINTER => *pointer.cast::<*mut c_void>() as usize as u64,
            _ => {
                return Err(format!(
                    "node:ffi callback has invalid argument type {kind}"
                ));
            }
        }
    };
    Ok(FfiValue {
        kind,
        reserved: 0,
        bits,
    })
}

unsafe fn write_callback_result(
    pointer: *mut c_void,
    expected: u32,
    value: FfiValue,
) -> Result<(), String> {
    if expected == TYPE_VOID {
        return Ok(());
    }
    if pointer.is_null() {
        return Err("node:ffi callback received a null result slot".to_owned());
    }
    if value.kind != expected || value.reserved != 0 {
        return Err(format!(
            "node:ffi callback returned kind {}, expected {expected}",
            value.kind
        ));
    }
    // SAFETY: libffi allocated the result slot for `expected`, and each write uses that exact type.
    unsafe {
        match expected {
            TYPE_I8 => pointer.cast::<i8>().write(value.bits as i8),
            TYPE_U8 | TYPE_BOOL => pointer.cast::<u8>().write(value.bits as u8),
            TYPE_I16 => pointer.cast::<i16>().write(value.bits as i16),
            TYPE_U16 => pointer.cast::<u16>().write(value.bits as u16),
            TYPE_I32 => pointer.cast::<i32>().write(value.bits as i32),
            TYPE_U32 => pointer.cast::<u32>().write(value.bits as u32),
            TYPE_I64 => pointer.cast::<i64>().write(value.bits as i64),
            TYPE_U64 => pointer.cast::<u64>().write(value.bits),
            TYPE_F32 => pointer
                .cast::<f32>()
                .write(f32::from_bits(value.bits as u32)),
            TYPE_F64 => pointer.cast::<f64>().write(f64::from_bits(value.bits)),
            TYPE_POINTER => pointer
                .cast::<*mut c_void>()
                .write(value.bits as usize as *mut c_void),
            _ => {
                return Err(format!(
                    "node:ffi callback has invalid result type {expected}"
                ));
            }
        }
    }
    Ok(())
}

struct ActiveCallback<'a>(&'a Cell<u32>);

impl Drop for ActiveCallback<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

unsafe extern "C" fn callback_trampoline(
    _cif: *mut low::ffi_cif,
    result: *mut c_void,
    arguments: *mut *mut c_void,
    userdata: *mut c_void,
) {
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
        if userdata.is_null() {
            return Err("node:ffi callback userdata is null".to_owned());
        }
        // SAFETY: registration gives libffi this stable boxed allocation until unregistration.
        let data = unsafe { &*userdata.cast::<CallbackData>() };
        // Leave a deterministic zero behind if normalization or the language callback fails.
        // Foreign callers may not understand our thread-local error channel and must never read
        // libffi's uninitialized return storage.
        unsafe {
            write_callback_result(
                result,
                data.result,
                FfiValue {
                    kind: data.result,
                    reserved: 0,
                    bits: 0,
                },
            )?
        };
        let active = data
            .active
            .get()
            .checked_add(1)
            .ok_or_else(|| "node:ffi callback recursion overflow".to_owned())?;
        data.active.set(active);
        let _active = ActiveCallback(&data.active);
        if !data.arguments.is_empty() && arguments.is_null() {
            return Err("node:ffi callback argument vector is null".to_owned());
        }
        let mut normalized = Vec::with_capacity(data.arguments.len());
        for (index, kind) in data.arguments.iter().copied().enumerate() {
            // SAFETY: libffi supplies one pointer per declared CIF argument.
            let argument = unsafe { *arguments.add(index) };
            // SAFETY: `kind` is the type used to build this callback's CIF.
            normalized.push(unsafe { callback_argument(argument, kind) }?);
        }
        let mut normalized_result = FfiValue {
            kind: data.result,
            reserved: 0,
            bits: 0,
        };
        // SAFETY: the C runtime owns `context` and the dispatch function for the whole registration.
        let status = unsafe {
            (data.dispatch)(
                data.context,
                normalized.as_ptr(),
                normalized.len() as u64,
                &mut normalized_result,
            )
        };
        if status != 0 {
            return Err(format!(
                "node:ffi callback dispatch failed with status {status}"
            ));
        }
        // SAFETY: the result slot belongs to this libffi callback invocation.
        unsafe { write_callback_result(result, data.result, normalized_result) }
    }));
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(message)) => {
            let _ = fail(message);
        }
        Err(_) => {
            let _ = fail("node:ffi callback provider panicked".to_owned());
        }
    }
}

/// Create a same-thread C-callable closure and return its executable pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_ffi_register_callback(
    argument_types: *const u32,
    argument_count: u64,
    result_type: u32,
    dispatch: Option<Dispatch>,
    context: *mut c_void,
    out_pointer: *mut u64,
) -> i32 {
    guarded(|| {
        let Some(dispatch) = dispatch else {
            return Err("node:ffi callback dispatch is null".to_owned());
        };
        if out_pointer.is_null() {
            return Err("node:ffi callback pointer output is null".to_owned());
        }
        // SAFETY: validated by `type_codes` against the C ABI contract.
        let argument_codes = unsafe { type_codes(argument_types, argument_count) }?.to_vec();
        let argument_ffi_types = argument_codes
            .iter()
            .map(|kind| ffi_type(*kind, true))
            .collect::<Result<Vec<_>, _>>()?;
        let result_ffi_type = ffi_type(result_type, false)?;
        let cif = Box::new(Cif::new(argument_ffi_types, result_ffi_type));
        let data = Box::new(CallbackData {
            dispatch,
            context,
            arguments: argument_codes,
            result: result_type,
            active: Cell::new(0),
        });
        let Some((allocation, code)) = low::try_closure_alloc() else {
            return Err("node:ffi could not allocate callback closure".to_owned());
        };
        // SAFETY: allocation/code are a pair from libffi; boxed CIF and userdata remain stable in
        // the Callback owner until its Drop frees the allocation.
        let status = unsafe {
            raw::ffi_prep_closure_loc(
                allocation,
                cif.as_raw_ptr(),
                Some(callback_trampoline),
                (&*data as *const CallbackData).cast_mut().cast(),
                code.as_mut_ptr(),
            )
        };
        if status != raw::ffi_status_FFI_OK {
            // SAFETY: preparation failed, but allocation still must be released exactly once.
            unsafe { low::closure_free(allocation) };
            return Err(format!(
                "node:ffi could not prepare callback closure: libffi status {status}"
            ));
        }
        let pointer = code.as_ptr() as usize as u64;
        with_state(|state| {
            if state.callbacks.contains_key(&pointer) {
                // `callback` has not been installed yet, so free its allocation here.
                // SAFETY: allocation is owned and has not previously been freed.
                unsafe { low::closure_free(allocation) };
                return Err("node:ffi callback pointer collision".to_owned());
            }
            state.callbacks.insert(
                pointer,
                Callback {
                    cif,
                    allocation,
                    data,
                },
            );
            // SAFETY: checked non-null and owned by the caller.
            unsafe { out_pointer.write(pointer) };
            Ok(())
        })
    })
}

/// Release a callback pointer after proving it is not currently executing.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_ffi_unregister_callback(pointer: u64) -> i32 {
    guarded(|| {
        with_state(|state| {
            let callback = state
                .callbacks
                .get(&pointer)
                .ok_or_else(|| format!("node:ffi callback pointer {pointer} is not registered"))?;
            if callback.data.active.get() != 0 {
                return Err(format!(
                    "node:ffi callback pointer {pointer} cannot unregister while executing"
                ));
            }
            state.callbacks.remove(&pointer);
            Ok(())
        })
    })
}

/// Borrow the most recent provider diagnostic on this thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_ffi_last_error(out_bytes: *mut *const u8, out_len: *mut u64) -> i32 {
    if out_bytes.is_null() || out_len.is_null() {
        return INVALID_ARGUMENT;
    }
    LAST_ERROR.with_borrow(|stored| {
        // SAFETY: both outputs are checked non-null. The bytes remain valid until the next FFI
        // provider operation on this same thread.
        unsafe {
            out_bytes.write(if stored.is_empty() {
                ptr::null()
            } else {
                stored.as_ptr()
            });
            out_len.write(stored.len() as u64);
        }
    });
    0
}

macro_rules! assert_provider_export {
    ($(($group:ident, $name:ident, $signature:ident);)*) => {
        $(assert_provider_export!($group, $name, $signature);)*
    };
    (ffi, kiln_ffi_open, ffi_open) => {
        const _: unsafe extern "C" fn(*const u8, u64, *mut u64) -> i32 = kiln_ffi_open;
    };
    (ffi, kiln_ffi_close, ffi_close) => {
        const _: extern "C" fn(u64) -> i32 = kiln_ffi_close;
    };
    (ffi, kiln_ffi_prepare, ffi_prepare) => {
        const _: unsafe extern "C" fn(u64, *const u8, u64, *const u32, u64, u32, *mut u64) -> i32 = kiln_ffi_prepare;
    };
    (ffi, kiln_ffi_call, ffi_call) => {
        const _: unsafe extern "C" fn(u64, *const FfiValue, u64, *mut FfiValue) -> i32 = kiln_ffi_call;
    };
    (ffi, kiln_ffi_register_callback, ffi_register_callback) => {
        const _: unsafe extern "C" fn(*const u32, u64, u32, Option<Dispatch>, *mut c_void, *mut u64) -> i32 = kiln_ffi_register_callback;
    };
    (ffi, kiln_ffi_unregister_callback, ffi_unregister_callback) => {
        const _: extern "C" fn(u64) -> i32 = kiln_ffi_unregister_callback;
    };
    (ffi, kiln_ffi_last_error, ffi_last_error) => {
        const _: unsafe extern "C" fn(*mut *const u8, *mut u64) -> i32 = kiln_ffi_last_error;
    };
    ($group:ident, $name:ident, $signature:ident) => {};
}

kiln_native_provider_contract::provider_abi_registry!(assert_provider_export);

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem;

    extern "C" fn weighted(left: i32, right: f64) -> f64 {
        f64::from(left) * 2.0 + right
    }

    extern "C" fn negate_i64(value: i64) -> i64 {
        -value
    }

    extern "C" fn invert_bool(value: u8) -> u8 {
        u8::from(value == 0)
    }

    #[test]
    fn prepared_cif_preserves_integer_and_float_arguments() {
        let function = Function {
            library: 0,
            pointer: weighted as *const () as *mut c_void,
            cif: Cif::new([Type::i32(), Type::f64()], Type::f64()),
            arguments: vec![TYPE_I32, TYPE_F64],
            result: TYPE_F64,
        };
        let result = call(
            &function,
            &[
                FfiValue {
                    kind: TYPE_I32,
                    reserved: 0,
                    bits: 7,
                },
                FfiValue {
                    kind: TYPE_F64,
                    reserved: 0,
                    bits: 0.5_f64.to_bits(),
                },
            ],
        );
        assert!(result.is_ok(), "valid fixed signature calls: {result:?}");
        let result = result.unwrap_or_default();
        assert_eq!(result.kind, TYPE_F64);
        assert_eq!(f64::from_bits(result.bits), 14.5);
    }

    #[test]
    fn prepared_cif_preserves_signed_i64_bits() {
        let function = Function {
            library: 0,
            pointer: negate_i64 as *const () as *mut c_void,
            cif: Cif::new([Type::i64()], Type::i64()),
            arguments: vec![TYPE_I64],
            result: TYPE_I64,
        };
        let result = call(
            &function,
            &[FfiValue {
                kind: TYPE_I64,
                reserved: 0,
                bits: (-42_i64) as u64,
            }],
        )
        .expect("valid signed call");
        assert_eq!(result.kind, TYPE_I64);
        assert_eq!(result.bits as i64, 42);
    }

    #[test]
    fn bool_has_a_distinct_value_kind() {
        let function = Function {
            library: 0,
            pointer: invert_bool as *const () as *mut c_void,
            cif: Cif::new([Type::u8()], Type::u8()),
            arguments: vec![TYPE_BOOL],
            result: TYPE_BOOL,
        };
        let result = call(
            &function,
            &[FfiValue {
                kind: TYPE_BOOL,
                reserved: 0,
                bits: 1,
            }],
        )
        .expect("valid boolean call");
        assert_eq!(result.kind, TYPE_BOOL);
        assert_eq!(result.bits, 0);
    }

    unsafe extern "C" fn add_callback(
        context: *mut c_void,
        arguments: *const FfiValue,
        argument_count: u64,
        result: *mut FfiValue,
    ) -> i32 {
        if context.is_null() || arguments.is_null() || result.is_null() || argument_count != 2 {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the callback test registers exactly two values and live output/context slots.
        unsafe {
            let arguments = slice::from_raw_parts(arguments, 2);
            let pointer = *(context.cast::<u64>());
            CALLBACK_UNREGISTER_STATUS
                .with(|status| status.set(kiln_ffi_unregister_callback(pointer)));
            let left = arguments.first().copied().unwrap_or_default();
            let right = arguments.get(1).copied().unwrap_or_default();
            result.write(FfiValue {
                kind: TYPE_I32,
                reserved: 0,
                bits: left.bits.wrapping_add(right.bits),
            });
        }
        0
    }

    thread_local! {
        static CALLBACK_UNREGISTER_STATUS: Cell<i32> = const { Cell::new(0) };
    }

    #[test]
    fn callback_closure_round_trips_and_refuses_self_unregistration() {
        let kinds = [TYPE_I32, TYPE_I32];
        let mut pointer = 0;
        // SAFETY: all registration inputs and outputs stay live, and the callback is invoked only
        // on this thread before unregistration.
        assert_eq!(
            unsafe {
                kiln_ffi_register_callback(
                    kinds.as_ptr(),
                    kinds.len() as u64,
                    TYPE_I32,
                    Some(add_callback),
                    (&mut pointer as *mut u64).cast(),
                    &mut pointer,
                )
            },
            0
        );
        // SAFETY: the provider created this pointer for exactly this fixed signature.
        let function: extern "C" fn(i32, i32) -> i32 = unsafe { mem::transmute(pointer as usize) };
        assert_eq!(function(17, 25), 42);
        CALLBACK_UNREGISTER_STATUS.with(|status| assert_eq!(status.get(), RUNTIME_ERROR));
        assert_eq!(kiln_ffi_unregister_callback(pointer), 0);
    }

    #[cfg(all(unix, target_pointer_width = "64"))]
    #[test]
    fn library_handles_resolve_call_and_invalidate_strlen() {
        #[cfg(target_os = "linux")]
        let library_name = b"libc.so.6";
        #[cfg(target_os = "macos")]
        let library_name = b"/usr/lib/libSystem.B.dylib";

        let mut library = 0;
        // SAFETY: all borrowed input and output storage lives through each provider call.
        assert_eq!(
            unsafe {
                kiln_ffi_open(
                    library_name.as_ptr(),
                    library_name.len() as u64,
                    &mut library,
                )
            },
            0
        );
        let kinds = [TYPE_POINTER];
        let mut function = 0;
        let symbol = b"strlen";
        // SAFETY: all borrowed input and output storage lives through the provider call.
        assert_eq!(
            unsafe {
                kiln_ffi_prepare(
                    library,
                    symbol.as_ptr(),
                    symbol.len() as u64,
                    kinds.as_ptr(),
                    kinds.len() as u64,
                    TYPE_U64,
                    &mut function,
                )
            },
            0
        );
        let text = b"same process\0";
        let argument = FfiValue {
            kind: TYPE_POINTER,
            reserved: 0,
            bits: text.as_ptr() as usize as u64,
        };
        let mut result = FfiValue::default();
        // SAFETY: the argument points at a live NUL-terminated byte string.
        assert_eq!(
            unsafe { kiln_ffi_call(function, &argument, 1, &mut result) },
            0
        );
        assert_eq!(
            result,
            FfiValue {
                kind: TYPE_U64,
                reserved: 0,
                bits: 12
            }
        );

        assert_eq!(kiln_ffi_close(library), 0);
        // SAFETY: stale integer handles are rejected without dereferencing anything.
        assert_eq!(
            unsafe { kiln_ffi_call(function, &argument, 1, &mut result) },
            RUNTIME_ERROR
        );
        let mut error = ptr::null();
        let mut error_len = 0;
        // SAFETY: both output pointers name live local storage.
        assert_eq!(
            unsafe { kiln_ffi_last_error(&mut error, &mut error_len) },
            0
        );
        // SAFETY: the diagnostic is borrowed until the next provider operation on this thread.
        let error = unsafe { slice::from_raw_parts(error, error_len as usize) };
        assert!(String::from_utf8_lossy(error).contains("is not prepared"));
    }
}
