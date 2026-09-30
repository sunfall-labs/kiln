#![allow(clippy::missing_safety_doc)]

use crate::{
    FileFilter, MessageOptions, NotificationOptions, OpenFileOptions, PickFolderOptions,
    SaveFileOptions, clipboard, dialog, notification, theme, tray,
};
use std::ffi::CStr;
use std::os::raw::c_char;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(f))
    } else {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("start tokio runtime for host GUI");
        rt.block_on(f)
    }
}

unsafe fn c_str_to_opt_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .ok()
            .map(|s| s.to_owned())
    }
}

unsafe fn copy_to_c_buf(s: &str, out_buf: *mut c_char, out_len: usize) -> i32 {
    if out_buf.is_null() || out_len == 0 {
        return -1;
    }
    let bytes = s.as_bytes();
    let to_copy = bytes.len().min(out_len - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out_buf as *mut u8, to_copy);
        *out_buf.add(to_copy) = 0;
    }
    to_copy as i32
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_dialog_open_file(
    title: *const c_char,
    filter_name: *const c_char,
    filter_exts: *const c_char,
    out_buf: *mut c_char,
    out_len: usize,
) -> i32 {
    let title = unsafe { c_str_to_opt_string(title) };
    let mut filters = None;
    if let Some(fname) = unsafe { c_str_to_opt_string(filter_name) } {
        let fexts = unsafe { c_str_to_opt_string(filter_exts) }.unwrap_or_default();
        let exts: Vec<String> = fexts
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        filters = Some(vec![FileFilter {
            name: fname,
            extensions: exts,
        }]);
    }
    let opts = OpenFileOptions { title, filters };
    let res = block_on(dialog::open_file(Some(opts)));
    match res {
        Some(path) => unsafe { copy_to_c_buf(&path, out_buf, out_len) },
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_dialog_save_file(
    title: *const c_char,
    default_path: *const c_char,
    filter_name: *const c_char,
    filter_exts: *const c_char,
    out_buf: *mut c_char,
    out_len: usize,
) -> i32 {
    let title = unsafe { c_str_to_opt_string(title) };
    let default_path = unsafe { c_str_to_opt_string(default_path) };
    let mut filters = None;
    if let Some(fname) = unsafe { c_str_to_opt_string(filter_name) } {
        let fexts = unsafe { c_str_to_opt_string(filter_exts) }.unwrap_or_default();
        let exts: Vec<String> = fexts
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        filters = Some(vec![FileFilter {
            name: fname,
            extensions: exts,
        }]);
    }
    let opts = SaveFileOptions {
        title,
        default_path,
        filters,
    };
    let res = block_on(dialog::save_file(Some(opts)));
    match res {
        Some(path) => unsafe { copy_to_c_buf(&path, out_buf, out_len) },
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_dialog_message(
    title: *const c_char,
    message: *const c_char,
    kind: *const c_char,
) -> i32 {
    let title = unsafe { c_str_to_opt_string(title) };
    let message = unsafe { c_str_to_opt_string(message) }.unwrap_or_default();
    let kind = unsafe { c_str_to_opt_string(kind) };
    let opts = MessageOptions {
        title,
        message,
        kind,
    };
    block_on(dialog::message(opts));
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_clipboard_read(out_buf: *mut c_char, out_len: usize) -> i32 {
    match clipboard::read_text() {
        Ok(text) => unsafe { copy_to_c_buf(&text, out_buf, out_len) },
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_clipboard_write(text: *const c_char) -> i32 {
    if text.is_null() {
        return -1;
    }
    let s = match unsafe { CStr::from_ptr(text) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    match clipboard::write_text(s) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_get_theme() -> i32 {
    match theme::get_theme() {
        "dark" => 0,
        _ => 1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_tray_create(tooltip: *const c_char) -> u32 {
    let tooltip = unsafe { c_str_to_opt_string(tooltip) }.unwrap_or_default();
    tray::create_tray_icon(&tooltip).unwrap_or_default()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_dialog_pick_folder(
    title: *const c_char,
    default_path: *const c_char,
    out_buf: *mut c_char,
    out_len: usize,
) -> i32 {
    let title = unsafe { c_str_to_opt_string(title) };
    let default_path = unsafe { c_str_to_opt_string(default_path) };
    let opts = PickFolderOptions {
        title,
        default_path,
    };
    let res = block_on(dialog::pick_folder(Some(opts)));
    match res {
        Some(path) => unsafe { copy_to_c_buf(&path, out_buf, out_len) },
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_clipboard_clear() -> i32 {
    match clipboard::clear() {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_notification_show(
    title: *const c_char,
    body: *const c_char,
    app_name: *const c_char,
) -> i32 {
    let title = unsafe { c_str_to_opt_string(title) }.unwrap_or_default();
    let body = unsafe { c_str_to_opt_string(body) };
    let app_name = unsafe { c_str_to_opt_string(app_name) };
    let opts = NotificationOptions {
        title,
        body,
        app_name,
    };
    match block_on(notification::show(opts)) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_tray_set_tooltip(id: u32, tooltip: *const c_char) -> i32 {
    let tooltip = unsafe { c_str_to_opt_string(tooltip) }.unwrap_or_default();
    match tray::set_tooltip(id, &tooltip) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_tray_destroy(id: u32) -> i32 {
    tray::destroy_tray_icon(id);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_tray_clear() -> i32 {
    tray::clear_tray_icons();
    0
}
