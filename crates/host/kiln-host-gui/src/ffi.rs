#![allow(clippy::missing_safety_doc)]

use crate::{
    FileFilter, MessageOptions, NotificationOptions, OpenFileOptions, PickFolderOptions,
    SaveFileOptions, app, clipboard, dialog, hotkey, notification, single_instance, theme, tray,
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_app_get_path(
    name: *const c_char,
    out_buf: *mut c_char,
    out_len: usize,
) -> i32 {
    let name_str = match unsafe { c_str_to_opt_string(name) } {
        Some(s) => s,
        None => return -1,
    };
    match app::get_path(&name_str) {
        Some(p) => unsafe { copy_to_c_buf(&p.to_string_lossy(), out_buf, out_len) },
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_app_single_instance_lock(app_id: *const c_char) -> i32 {
    let app_id = unsafe { c_str_to_opt_string(app_id) }.unwrap_or_else(|| "default".to_string());
    if single_instance::request_single_instance_lock(&app_id) {
        0
    } else {
        -1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_hotkey_register(accelerator: *const c_char) -> u32 {
    let acc = match unsafe { c_str_to_opt_string(accelerator) } {
        Some(s) => s,
        None => return 0,
    };
    hotkey::register(&acc).unwrap_or_default()
}

#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_hotkey_unregister(id: u32) -> i32 {
    match hotkey::unregister(id) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_hotkey_poll() -> u32 {
    hotkey::poll_event().unwrap_or(0)
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_appkit_init() -> i32 {
    if crate::appkit::ensure_initialized() {
        0
    } else {
        -1
    }
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_appkit_window_create(
    id: u64,
    title: *const c_char,
    width: f64,
    height: f64,
) -> i32 {
    let title_str =
        unsafe { c_str_to_opt_string(title) }.unwrap_or_else(|| "Kiln Window".to_string());
    if crate::appkit::window_create(id, &title_str, width, height) {
        0
    } else {
        -1
    }
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_appkit_window_show(id: u64) -> i32 {
    crate::appkit::window_show(id);
    0
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_appkit_window_close(id: u64) -> i32 {
    crate::appkit::window_close(id);
    0
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_appkit_window_set_title(id: u64, title: *const c_char) -> i32 {
    if let Some(t) = unsafe { c_str_to_opt_string(title) } {
        crate::appkit::window_set_title(id, &t);
    }
    0
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_appkit_window_set_content(win_id: u64, child_id: u64) -> i32 {
    crate::appkit::window_set_content(win_id, child_id);
    0
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_appkit_widget_create(
    id: u64,
    tag: *const c_char,
    text: *const c_char,
) -> i32 {
    let tag_str = unsafe { c_str_to_opt_string(tag) }.unwrap_or_default();
    let text_str = unsafe { c_str_to_opt_string(text) }.unwrap_or_default();
    if crate::appkit::widget_create(id, &tag_str, &text_str) {
        0
    } else {
        -1
    }
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_appkit_widget_set_property(
    id: u64,
    name: *const c_char,
    val_str: *const c_char,
    val_num: f64,
) -> i32 {
    let name_str = unsafe { c_str_to_opt_string(name) }.unwrap_or_default();
    let str_val = unsafe { c_str_to_opt_string(val_str) }.unwrap_or_default();
    crate::appkit::widget_set_property(id, &name_str, &str_val, val_num);
    0
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_appkit_widget_add_child(parent_id: u64, child_id: u64) -> i32 {
    crate::appkit::widget_add_child(parent_id, child_id);
    0
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_appkit_widget_remove_child(parent_id: u64, child_id: u64) -> i32 {
    crate::appkit::widget_remove_child(parent_id, child_id);
    0
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_appkit_pump_events(wait_ms: f64) -> i32 {
    if crate::appkit::pump_events(wait_ms) {
        1
    } else {
        0
    }
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_gui_appkit_poll_event(
    out_id: *mut u64,
    out_type_buf: *mut c_char,
    out_type_len: usize,
    out_val_buf: *mut c_char,
    out_val_len: usize,
) -> i32 {
    if let Some((id, event_type, val)) = crate::appkit::poll_event() {
        if !out_id.is_null() {
            unsafe { *out_id = id };
        }
        unsafe {
            copy_to_c_buf(&event_type, out_type_buf, out_type_len);
            if !out_val_buf.is_null() && out_val_len > 0 {
                copy_to_c_buf(&val, out_val_buf, out_val_len);
            }
        };
        1
    } else {
        0
    }
}

#[cfg(not(target_os = "macos"))]
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_appkit_init() -> i32 {
    -1
}
#[cfg(not(target_os = "macos"))]
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_appkit_window_create(_: u64, _: *const c_char, _: f64, _: f64) -> i32 {
    -1
}
#[cfg(not(target_os = "macos"))]
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_appkit_window_show(_: u64) -> i32 {
    -1
}
#[cfg(not(target_os = "macos"))]
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_appkit_window_close(_: u64) -> i32 {
    -1
}
#[cfg(not(target_os = "macos"))]
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_appkit_window_set_title(_: u64, _: *const c_char) -> i32 {
    -1
}
#[cfg(not(target_os = "macos"))]
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_appkit_window_set_content(_: u64, _: u64) -> i32 {
    -1
}
#[cfg(not(target_os = "macos"))]
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_appkit_widget_create(_: u64, _: *const c_char, _: *const c_char) -> i32 {
    -1
}
#[cfg(not(target_os = "macos"))]
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_appkit_widget_set_property(
    _: u64,
    _: *const c_char,
    _: *const c_char,
    _: f64,
) -> i32 {
    -1
}
#[cfg(not(target_os = "macos"))]
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_appkit_widget_add_child(_: u64, _: u64) -> i32 {
    -1
}
#[cfg(not(target_os = "macos"))]
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_appkit_widget_remove_child(_: u64, _: u64) -> i32 {
    -1
}
#[cfg(not(target_os = "macos"))]
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_appkit_pump_events(_: f64) -> i32 {
    0
}
#[cfg(not(target_os = "macos"))]
#[unsafe(no_mangle)]
pub extern "C" fn kiln_gui_appkit_poll_event(
    _: *mut u64,
    _: *mut c_char,
    _: usize,
    _: *mut c_char,
    _: usize,
) -> i32 {
    0
}
