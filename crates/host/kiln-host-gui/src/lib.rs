//! The native half of kiln:gui (ADR 0074): widgets, layout and OS services behind one C ABI.
//!
//! * [`ffi`] is the ABI the TypeScript library binds with `dlopen` (`kiln_gui_*`, version
//!   [`ffi::ABI_VERSION`]).
//! * `appkit` is the macOS widget backend and `gtk` the Linux one (GTK 4, behind the default
//!   `gtk` feature). Other platforms report that they have none, and the TypeScript side falls
//!   back with that reason rather than pretending.
//! * [`layout`] parses the style objects the TypeScript side sends and keeps the Taffy tree.
//! * [`events`] is the queue between toolkit callbacks and the program.
//! * The modules below are OS services that do not need a widget backend: dialogs, clipboard,
//!   theme, notifications, tray icons, global shortcuts, well-known paths, single-instance.

use serde::Deserialize;
use serde_json::{Value, json};

#[cfg(target_os = "macos")]
pub mod appkit;
pub mod events;
pub mod ffi;
#[cfg(all(target_os = "linux", feature = "gtk"))]
pub mod gtk;
pub mod layout;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

/// Every dialog's options; each kind reads the fields it understands.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogOptions {
    pub title: Option<String>,
    pub message: Option<String>,
    pub detail: Option<String>,
    pub default_path: Option<String>,
    pub file_name: Option<String>,
    pub filters: Option<Vec<FileFilter>>,
    pub multiple: Option<bool>,
    pub kind: Option<String>,
}

pub mod dialog {
    use super::*;
    use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

    fn file_dialog(options: &DialogOptions) -> FileDialog {
        let mut d = FileDialog::new();
        if let Some(title) = &options.title {
            d = d.set_title(title);
        }
        if let Some(dir) = &options.default_path {
            d = d.set_directory(dir);
        }
        if let Some(name) = &options.file_name {
            d = d.set_file_name(name);
        }
        for filter in options.filters.iter().flatten() {
            let exts: Vec<&str> = filter.extensions.iter().map(String::as_str).collect();
            d = d.add_filter(&filter.name, &exts);
        }
        d
    }

    fn path(p: std::path::PathBuf) -> Value {
        Value::String(p.to_string_lossy().into_owned())
    }

    /// Runs a modal dialog. The result is JSON: a path, a list of paths, `true`/`false` for a
    /// confirmation, or `null` when the person cancelled.
    pub fn run(kind: &str, options: &DialogOptions) -> Result<Value, String> {
        Ok(match kind {
            "open" if options.multiple == Some(true) => file_dialog(options)
                .pick_files()
                .map(|paths| Value::Array(paths.into_iter().map(path).collect()))
                .unwrap_or(Value::Null),
            "open" => file_dialog(options)
                .pick_file()
                .map(path)
                .unwrap_or(Value::Null),
            "save" => file_dialog(options)
                .save_file()
                .map(path)
                .unwrap_or(Value::Null),
            "folder" => file_dialog(options)
                .pick_folder()
                .map(path)
                .unwrap_or(Value::Null),
            "message" | "confirm" => {
                let level = match options.kind.as_deref() {
                    Some("warning") => MessageLevel::Warning,
                    Some("error") => MessageLevel::Error,
                    _ => MessageLevel::Info,
                };
                let mut d = MessageDialog::new().set_level(level).set_description(
                    options
                        .detail
                        .clone()
                        .or_else(|| options.message.clone())
                        .unwrap_or_default(),
                );
                if let Some(title) = options
                    .title
                    .clone()
                    .or_else(|| options.message.clone().filter(|_| options.detail.is_some()))
                {
                    d = d.set_title(title);
                }
                if kind == "confirm" {
                    d = d.set_buttons(MessageButtons::OkCancel);
                }
                let result = d.show();
                json!(matches!(
                    result,
                    MessageDialogResult::Ok | MessageDialogResult::Yes
                ))
            }
            other => return Err(format!("unknown dialog `{other}`")),
        })
    }
}

pub mod clipboard {
    pub fn read_text() -> Result<String, String> {
        arboard::Clipboard::new()
            .and_then(|mut cb| cb.get_text())
            .map_err(|e| e.to_string())
    }

    pub fn write_text(text: &str) -> Result<(), String> {
        arboard::Clipboard::new()
            .and_then(|mut cb| cb.set_text(text))
            .map_err(|e| e.to_string())
    }

    pub fn clear() -> Result<(), String> {
        arboard::Clipboard::new()
            .and_then(|mut cb| cb.clear())
            .map_err(|e| e.to_string())
    }
}

pub mod theme {
    pub fn is_dark() -> bool {
        matches!(dark_light::detect(), dark_light::Mode::Dark)
    }
}

pub mod notification {
    /// Posts a desktop notification. An unbundled binary cannot use the macOS notification
    /// center directly, so macOS goes through `osascript`, which can.
    pub fn show(title: &str, body: &str, app_name: &str) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
            let script = format!(
                "display notification \"{}\" with title \"{}\"",
                quote(body),
                quote(if title.is_empty() { app_name } else { title })
            );
            let status = std::process::Command::new("osascript")
                .args(["-e", &script])
                .status()
                .map_err(|e| e.to_string())?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("osascript exited with {status}"))
            }
        }
        #[cfg(target_os = "linux")]
        {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            runtime.block_on(async {
                let connection = zbus::Connection::session()
                    .await
                    .map_err(|e| e.to_string())?;
                let actions: &[&str] = &[];
                let hints = std::collections::HashMap::<&str, zbus::zvariant::Value>::new();
                connection
                    .call_method(
                        Some("org.freedesktop.Notifications"),
                        "/org/freedesktop/Notifications",
                        Some("org.freedesktop.Notifications"),
                        "Notify",
                        &(app_name, 0u32, "", title, body, actions, hints, -1i32),
                    )
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = (title, body, app_name);
            Err("notifications are not implemented on this platform".into())
        }
    }
}

/// Builds a muda menu from the same JSON shape as the menu bar.
fn context_menu(items: &[Value]) -> muda::Menu {
    fn label(item: &Value) -> String {
        item.get("label")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
    fn fill(target: &dyn Fn(&dyn muda::IsMenuItem), items: &[Value]) {
        for item in items {
            if item.get("separator").and_then(Value::as_bool) == Some(true) {
                target(&muda::PredefinedMenuItem::separator());
            } else if let Some(Value::Array(children)) = item.get("items") {
                let sub = muda::Submenu::new(label(item), true);
                fill(
                    &|child| {
                        let _ = sub.append(child);
                    },
                    children,
                );
                target(&sub);
            } else {
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| label(item));
                let enabled = item.get("enabled").and_then(Value::as_bool).unwrap_or(true);
                let accelerator = item
                    .get("accelerator")
                    .and_then(Value::as_str)
                    .and_then(|a| a.parse().ok());
                if let Some(checked) = item.get("checked").and_then(Value::as_bool) {
                    target(&muda::CheckMenuItem::with_id(
                        id,
                        label(item),
                        enabled,
                        checked,
                        accelerator,
                    ));
                } else {
                    target(&muda::MenuItem::with_id(
                        id,
                        label(item),
                        enabled,
                        accelerator,
                    ));
                }
            }
        }
    }
    let menu = muda::Menu::new();
    fill(
        &|item| {
            let _ = menu.append(item);
        },
        items,
    );
    menu
}

pub mod tray {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use tray_icon::{TrayIcon, TrayIconBuilder};

    thread_local! {
        static TRAYS: RefCell<HashMap<u32, TrayIcon>> = RefCell::new(HashMap::new());
        static NEXT: RefCell<u32> = const { RefCell::new(1) };
    }

    /// `{ title, tooltip, menu: [...] }`. Returns the tray's id.
    pub fn create(spec: &Value) -> Result<u32, String> {
        let id = NEXT.with(|n| {
            let mut n = n.borrow_mut();
            *n += 1;
            *n - 1
        });
        let mut builder = TrayIconBuilder::new().with_id(id.to_string());
        if let Some(title) = spec.get("title").and_then(Value::as_str) {
            builder = builder.with_title(title);
        }
        if let Some(tooltip) = spec.get("tooltip").and_then(Value::as_str) {
            builder = builder.with_tooltip(tooltip);
        }
        if let Some(Value::Array(items)) = spec.get("menu") {
            builder = builder.with_menu(Box::new(context_menu(items)));
        }
        let tray = builder.build().map_err(|e| e.to_string())?;
        TRAYS.with(|t| t.borrow_mut().insert(id, tray));
        Ok(id)
    }

    pub fn update(id: u32, spec: &Value) -> Result<(), String> {
        TRAYS.with(|t| {
            let trays = t.borrow();
            let tray = trays.get(&id).ok_or_else(|| format!("no tray {id}"))?;
            if let Some(title) = spec.get("title").and_then(Value::as_str) {
                tray.set_title(Some(title));
            }
            if let Some(tooltip) = spec.get("tooltip").and_then(Value::as_str) {
                tray.set_tooltip(Some(tooltip)).map_err(|e| e.to_string())?;
            }
            if let Some(Value::Array(items)) = spec.get("menu") {
                tray.set_menu(Some(Box::new(context_menu(items))));
            }
            Ok(())
        })
    }

    pub fn destroy(id: u32) {
        TRAYS.with(|t| t.borrow_mut().remove(&id));
    }
}

pub mod app {
    use std::path::PathBuf;

    pub fn path(name: &str) -> Option<PathBuf> {
        match name {
            "home" => dirs::home_dir(),
            "config" | "appData" => dirs::config_dir(),
            "cache" => dirs::cache_dir(),
            "data" | "userData" => dirs::data_dir(),
            "desktop" => dirs::desktop_dir(),
            "documents" => dirs::document_dir(),
            "downloads" => dirs::download_dir(),
            "temp" => Some(std::env::temp_dir()),
            _ => None,
        }
    }
}

pub mod single_instance {
    use std::cell::RefCell;

    thread_local! {
        static HOLDER: RefCell<Option<Box<dyn std::any::Any>>> = const { RefCell::new(None) };
    }

    /// True when this process is the only one holding `app_id`.
    pub fn acquire(app_id: &str) -> bool {
        if HOLDER.with(|h| h.borrow().is_some()) {
            return true;
        }
        #[cfg(unix)]
        {
            use std::os::unix::net::{UnixListener, UnixStream};
            let safe: String = app_id
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let path = std::env::temp_dir().join(format!("kiln-app-{safe}.sock"));
            if UnixStream::connect(&path).is_ok() {
                return false;
            }
            let _ = std::fs::remove_file(&path);
            match UnixListener::bind(&path) {
                Ok(listener) => {
                    HOLDER.with(|h| *h.borrow_mut() = Some(Box::new(listener)));
                    true
                }
                Err(_) => false,
            }
        }
        #[cfg(not(unix))]
        {
            let _ = app_id;
            true
        }
    }
}

pub mod hotkey {
    use global_hotkey::{GlobalHotKeyManager, hotkey::HotKey};
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static MANAGER: RefCell<Option<GlobalHotKeyManager>> = const { RefCell::new(None) };
        static KEYS: RefCell<HashMap<u32, HotKey>> = RefCell::new(HashMap::new());
    }

    pub fn register(accelerator: &str) -> Result<u32, String> {
        let hotkey: HotKey = accelerator.parse().map_err(|e| format!("{e}"))?;
        MANAGER.with(|m| {
            let mut m = m.borrow_mut();
            if m.is_none() {
                *m = Some(GlobalHotKeyManager::new().map_err(|e| e.to_string())?);
            }
            m.as_ref()
                .expect("manager")
                .register(hotkey)
                .map_err(|e| e.to_string())
        })?;
        KEYS.with(|k| k.borrow_mut().insert(hotkey.id(), hotkey));
        Ok(hotkey.id())
    }

    pub fn unregister(id: u32) {
        if let Some(hotkey) = KEYS.with(|k| k.borrow_mut().remove(&id)) {
            MANAGER.with(|m| {
                if let Some(m) = m.borrow().as_ref() {
                    let _ = m.unregister(hotkey);
                }
            });
        }
    }
}

/// Moves tray, tray-menu and global-shortcut notifications into the event queue. Those crates
/// deliver through process-wide channels; a key press arrives as a press and a release, and only
/// the press is an event for the program.
pub fn poll_services() {
    use events::{Event, EventKind};
    while let Ok(event) = global_hotkey::GlobalHotKeyEvent::receiver().try_recv() {
        if event.state == global_hotkey::HotKeyState::Pressed {
            events::push(Event {
                handle: 0,
                kind: EventKind::Hotkey,
                a: event.id as f64,
                b: 0.0,
                text: String::new(),
            });
        }
    }
    while let Ok(event) = muda::MenuEvent::receiver().try_recv() {
        events::push(Event {
            handle: 0,
            kind: EventKind::Menu,
            a: 0.0,
            b: 0.0,
            text: event.id.0,
        });
    }
    while let Ok(event) = tray_icon::TrayIconEvent::receiver().try_recv() {
        if let tray_icon::TrayIconEvent::Click {
            id,
            button_state: tray_icon::MouseButtonState::Up,
            ..
        } = event
        {
            let tray = id.0.parse::<f64>().unwrap_or(0.0);
            events::push(Event {
                handle: 0,
                kind: EventKind::Tray,
                a: tray,
                b: 0.0,
                text: String::new(),
            });
        }
    }
}
