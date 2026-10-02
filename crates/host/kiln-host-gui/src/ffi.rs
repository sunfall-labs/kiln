//! The C ABI the TypeScript library binds with `dlopen` (ADR 0074).
//!
//! Conventions, so the binding side needs no per-function knowledge:
//!
//! * Strings in are NUL-terminated UTF-8. Strings out are not returned directly: a call that
//!   produces text stores it and returns its byte length, and `kiln_gui_result` copies it out.
//! * A negative `i32` is a failure. `-1` means "nothing" (a cancelled dialog, an empty
//!   clipboard); `-2` means an error whose message `kiln_gui_result` copies out.
//! * Handles are `u32`, allocated here, never by the caller. `0` is "none".
//! * Nothing here unwinds into the caller: a panic becomes a `-2` with its message.

#![allow(clippy::missing_safety_doc)]

use std::cell::RefCell;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::Value;

use crate::events::{self, EventHeader};
use crate::{
    DialogOptions, app, clipboard, dialog, hotkey, notification, single_instance, theme, tray,
};

/// Bumped on any change a binding must know about; the TypeScript side refuses a mismatch.
pub const ABI_VERSION: u32 = 4;

thread_local! {
    static RESULT: RefCell<String> = const { RefCell::new(String::new()) };
}

fn stash(text: String) -> i32 {
    let len = text.len().min(i32::MAX as usize) as i32;
    RESULT.with(|r| *r.borrow_mut() = text);
    len
}

fn fail(message: impl Into<String>) -> i32 {
    stash(message.into());
    -2
}

fn guard(f: impl FnOnce() -> Result<i32, String>) -> i32 {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(code)) => code,
        Ok(Err(message)) => fail(message),
        Err(panic) => {
            let message = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            fail(format!("kiln-host-gui panicked: {message}"))
        }
    }
}

unsafe fn arg(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(target_os = "macos")]
use crate::appkit as backend;

#[cfg(not(target_os = "macos"))]
mod backend {
    const NONE: &str = "kiln-host-gui has no widget backend for this platform yet";
    pub fn init() -> Result<(), String> {
        Err(NONE.into())
    }
    pub fn create(_: &str) -> Result<u32, String> {
        Err(NONE.into())
    }
    pub fn set_text(_: u32, _: &str, _: &str) -> Result<(), String> {
        Err(NONE.into())
    }
    pub fn set_number(_: u32, _: &str, _: f64) -> Result<(), String> {
        Err(NONE.into())
    }
    pub fn insert(_: u32, _: u32, _: u32) -> Result<(), String> {
        Err(NONE.into())
    }
    pub fn remove(_: u32, _: u32) -> Result<(), String> {
        Err(NONE.into())
    }
    pub fn destroy(_: u32) -> Result<(), String> {
        Err(NONE.into())
    }
    pub fn call(_: u32, _: &str, _: &str) -> Result<i32, String> {
        Err(NONE.into())
    }
    pub fn wait(_: f64) -> Result<bool, String> {
        Err(NONE.into())
    }
    pub fn set_menu(_: &str) -> Result<(), String> {
        Err(NONE.into())
    }
    pub fn describe(_: u32) -> Result<String, String> {
        Err(NONE.into())
    }
    pub fn wake() {}
}

#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_abi_version() -> u32 {
    ABI_VERSION
}

/// Copies the text of the last result or error into `buf` and returns the bytes copied.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_result(buf: *mut u8, len: usize) -> i32 {
    RESULT.with(|r| {
        let r = r.borrow();
        let n = r.len().min(len);
        if n > 0 && !buf.is_null() {
            unsafe { std::ptr::copy_nonoverlapping(r.as_ptr(), buf, n) };
        }
        n as i32
    })
}

// --- Widgets ---------------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_init() -> i32 {
    guard(|| backend::init().map(|()| 0))
}

/// Creates a widget of `kind` and returns its handle, or 0 with the error in the result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_create(kind: *const c_char) -> u32 {
    let kind = unsafe { arg(kind) };
    let mut handle = 0;
    guard(|| {
        handle = backend::create(&kind)?;
        Ok(0)
    });
    handle
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_set_text(
    handle: u32,
    name: *const c_char,
    value: *const c_char,
) -> i32 {
    let (name, value) = unsafe { (arg(name), arg(value)) };
    guard(|| backend::set_text(handle, &name, &value).map(|()| 0))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_set_number(handle: u32, name: *const c_char, value: f64) -> i32 {
    let name = unsafe { arg(name) };
    guard(|| backend::set_number(handle, &name, value).map(|()| 0))
}

/// Inserts `child` into `parent` before `before` (0: at the end). Moves an attached child.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_insert(parent: u32, child: u32, before: u32) -> i32 {
    guard(|| backend::insert(parent, child, before).map(|()| 0))
}

#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_remove(parent: u32, child: u32) -> i32 {
    guard(|| backend::remove(parent, child).map(|()| 0))
}

#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_destroy(handle: u32) -> i32 {
    guard(|| backend::destroy(handle).map(|()| 0))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_call(
    handle: u32,
    command: *const c_char,
    arg_text: *const c_char,
) -> i32 {
    let (command, arg_text) = unsafe { (arg(command), arg(arg_text)) };
    guard(|| backend::call(handle, &command, &arg_text))
}

/// What AppKit holds for `handle`'s assistive technology (JSON: label, hint, element, focused).
/// Returns the length of the text `kiln_gui_result` copies out.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_describe(handle: u32) -> i32 {
    guard(|| backend::describe(handle).map(stash))
}

// --- Parking the realm in the run loop -------------------------------------------------------

/// How the kiln runtime installs a park hook (`kiln_runtime::set_park_hook`), passed in by the
/// native provider when it links this library in. A setter rather than a dependency: this crate
/// stays free of the runtime, and a separately loaded copy of it (the development dylib) simply
/// has none, so parking is unavailable there and the program keeps polling.
pub type ParkSetter = extern "C" fn(
    wait: Option<extern "C" fn(f64) -> i32>,
    wake: Option<extern "C" fn()>,
    port: u64,
);

static PARK_SETTER: std::sync::OnceLock<ParkSetter> = std::sync::OnceLock::new();

/// Called once by the provider that links this library into a kiln binary.
pub fn bind_park_setter(setter: ParkSetter) {
    let _ = PARK_SETTER.set(setter);
}

extern "C" fn park_wait(timeout_ms: f64) -> i32 {
    match backend::wait(timeout_ms) {
        Ok(ready) => {
            crate::poll_services();
            i32::from(ready || !events::is_empty())
        }
        Err(_) => 0,
    }
}

extern "C" fn park_wake() {
    backend::wake();
}

/// Makes the realm on this thread park inside the GUI run loop (ADR 0074): OS events, timers and
/// completions then end its wait without polling, and events are announced on worker `port`.
/// Returns 1 when installed, 0 when this copy of the library is not linked into a kiln binary.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_park(port: u64) -> i32 {
    match PARK_SETTER.get() {
        Some(set) => {
            set(Some(park_wait), Some(park_wake), port);
            1
        }
        None => 0,
    }
}

/// Removes the park hook (the last window closed): the realm parks in its own loop again.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_unpark() -> i32 {
    if let Some(set) = PARK_SETTER.get() {
        set(None, None, 0);
    }
    0
}

// --- Events ----------------------------------------------------------------------------------

/// Processes OS events for up to `timeout_ms` (returning early once any arrive). Returns 1 when
/// the program has events to read.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_wait(timeout_ms: f64) -> i32 {
    guard(|| {
        let ready = backend::wait(timeout_ms)?;
        crate::poll_services();
        Ok(i32::from(ready || !events::is_empty()))
    })
}

/// Writes the next event's header to `out` and returns 1, or returns 0 when there is none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_next_event(out: *mut EventHeader) -> i32 {
    crate::poll_services();
    match events::next() {
        Some(header) if !out.is_null() => {
            unsafe { *out = header };
            stash(events::current_text());
            1
        }
        _ => 0,
    }
}

// --- Services --------------------------------------------------------------------------------

/// `open`, `save`, `folder`, `message`, `confirm`. Returns the result's JSON length, or -1 when
/// the person cancelled.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_dialog(kind: *const c_char, options: *const c_char) -> i32 {
    let (kind, options) = unsafe { (arg(kind), arg(options)) };
    guard(|| {
        let options: DialogOptions = if options.is_empty() {
            DialogOptions::default()
        } else {
            serde_json::from_str(&options).map_err(|e| format!("dialog options: {e}"))?
        };
        match dialog::run(&kind, &options)? {
            Value::Null => Ok(-1),
            value => Ok(stash(value.to_string())),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_clipboard_read() -> i32 {
    guard(|| clipboard::read_text().map(stash).or(Ok(-1)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_clipboard_write(text: *const c_char) -> i32 {
    let text = unsafe { arg(text) };
    guard(|| clipboard::write_text(&text).map(|()| 0))
}

#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_clipboard_clear() -> i32 {
    guard(|| clipboard::clear().map(|()| 0))
}

/// 1 when the system appearance is dark.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_is_dark() -> i32 {
    guard(|| Ok(i32::from(theme::is_dark())))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_notify(
    title: *const c_char,
    body: *const c_char,
    app_name: *const c_char,
) -> i32 {
    let (title, body, app_name) = unsafe { (arg(title), arg(body), arg(app_name)) };
    guard(|| notification::show(&title, &body, &app_name).map(|()| 0))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_app_path(name: *const c_char) -> i32 {
    let name = unsafe { arg(name) };
    guard(|| {
        let path = app::path(&name).ok_or_else(|| format!("no well-known path named `{name}`"))?;
        Ok(stash(path.to_string_lossy().into_owned()))
    })
}

/// 1 when this process now holds `app_id`, 0 when another process does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_single_instance(app_id: *const c_char) -> i32 {
    let app_id = unsafe { arg(app_id) };
    guard(|| Ok(i32::from(single_instance::acquire(&app_id))))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_set_menu(spec: *const c_char) -> i32 {
    let spec = unsafe { arg(spec) };
    guard(|| backend::set_menu(&spec).map(|()| 0))
}

fn parse(spec: &str) -> Result<Value, String> {
    serde_json::from_str(spec).map_err(|e| format!("tray: {e}"))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_tray_create(spec: *const c_char) -> u32 {
    let spec = unsafe { arg(spec) };
    let mut id = 0;
    guard(|| {
        id = tray::create(&parse(&spec)?)?;
        Ok(0)
    });
    id
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_tray_update(id: u32, spec: *const c_char) -> i32 {
    let spec = unsafe { arg(spec) };
    guard(|| tray::update(id, &parse(&spec)?).map(|()| 0))
}

#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_tray_destroy(id: u32) -> i32 {
    guard(|| {
        tray::destroy(id);
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_hotkey_register(accelerator: *const c_char) -> u32 {
    let accelerator = unsafe { arg(accelerator) };
    let mut id = 0;
    guard(|| {
        id = hotkey::register(&accelerator)?;
        Ok(0)
    });
    id
}

#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_hotkey_unregister(id: u32) -> i32 {
    guard(|| {
        hotkey::unregister(id);
        Ok(0)
    })
}

/// Every `kiln_gui_*` operation this crate exports, by its C name, with its code pointer.
///
/// The native provider answers `kiln_ffi_open("kiln:host-gui")` from this table instead of
/// `dlopen`ing a file, so the bridge is linked into the program and cannot go missing at run time
/// (ADR 0074). Taking each function's address here is also what keeps it alive through the
/// provider archive's whole-archive LTO: an upstream `#[no_mangle]` that nothing references is
/// dropped from a staticlib, measured, with every framework it would have pulled in.
///
/// The list is held to the definitions above by `exports_match_the_definitions` below, and to the
/// contract's `GUI_EXPORTS` by the contract crate's own test over this same source file.
pub fn exports() -> Vec<(&'static str, *const core::ffi::c_void)> {
    macro_rules! table {
        ($($name:ident),* $(,)?) => {
            vec![$((stringify!($name), $name as *const () as *const core::ffi::c_void)),*]
        };
    }
    table![
        kiln_gui_abi_version,
        kiln_gui_result,
        kiln_gui_init,
        kiln_gui_create,
        kiln_gui_set_text,
        kiln_gui_set_number,
        kiln_gui_insert,
        kiln_gui_remove,
        kiln_gui_destroy,
        kiln_gui_call,
        kiln_gui_wait,
        kiln_gui_next_event,
        kiln_gui_dialog,
        kiln_gui_clipboard_read,
        kiln_gui_clipboard_write,
        kiln_gui_clipboard_clear,
        kiln_gui_is_dark,
        kiln_gui_notify,
        kiln_gui_app_path,
        kiln_gui_single_instance,
        kiln_gui_set_menu,
        kiln_gui_tray_create,
        kiln_gui_tray_update,
        kiln_gui_tray_destroy,
        kiln_gui_hotkey_register,
        kiln_gui_hotkey_unregister,
        kiln_gui_describe,
        kiln_gui_park,
        kiln_gui_unpark,
    ]
}

#[cfg(test)]
mod export_table_tests {
    #[test]
    fn exports_match_the_definitions() {
        let source = include_str!("ffi.rs");
        let mut defined = Vec::new();
        for line in source.lines() {
            let line = line.trim_start();
            let Some(rest) = line
                .strip_prefix("pub unsafe extern \"C\" fn ")
                .or_else(|| line.strip_prefix("pub extern \"C\" fn "))
            else {
                continue;
            };
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if name.starts_with("kiln_gui_") && !defined.contains(&name) {
                defined.push(name);
            }
        }
        defined.sort();
        let exports = super::exports();
        let mut listed: Vec<String> = exports.iter().map(|(name, _)| (*name).to_owned()).collect();
        listed.sort();
        assert_eq!(
            listed, defined,
            "exports() must name every kiln_gui_* definition once"
        );
        assert!(exports.iter().all(|(_, pointer)| !pointer.is_null()));
    }
}
