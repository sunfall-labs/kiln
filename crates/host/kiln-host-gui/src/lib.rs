//! Host GUI and OS utility capability engine for Kiln (ADR 0074).
//! Provides cross-platform desktop dialogs, clipboard, system theme detection,
//! native menus, and system tray integration.

use serde::{Deserialize, Serialize};

pub mod ffi;

// OS Dialog Options
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OpenFileOptions {
    pub title: Option<String>,
    pub filters: Option<Vec<FileFilter>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SaveFileOptions {
    pub title: Option<String>,
    pub default_path: Option<String>,
    pub filters: Option<Vec<FileFilter>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MessageOptions {
    pub title: Option<String>,
    pub message: String,
    pub kind: Option<String>, // "info" | "warning" | "error"
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PickFolderOptions {
    pub title: Option<String>,
    pub default_path: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NotificationOptions {
    pub title: String,
    pub body: Option<String>,
    pub app_name: Option<String>,
}

pub mod dialog {
    use super::*;

    pub async fn open_file(options: Option<OpenFileOptions>) -> Option<String> {
        let mut d = rfd::AsyncFileDialog::new();
        if let Some(opts) = options {
            if let Some(title) = opts.title {
                d = d.set_title(&title);
            }
            if let Some(filters) = opts.filters {
                for filter in filters {
                    let exts: Vec<&str> = filter.extensions.iter().map(|s| s.as_str()).collect();
                    d = d.add_filter(&filter.name, &exts);
                }
            }
        }
        let handle = d.pick_file().await?;
        Some(handle.path().to_string_lossy().into_owned())
    }

    pub async fn open_files(options: Option<OpenFileOptions>) -> Vec<String> {
        let mut d = rfd::AsyncFileDialog::new();
        if let Some(opts) = options {
            if let Some(title) = opts.title {
                d = d.set_title(&title);
            }
            if let Some(filters) = opts.filters {
                for filter in filters {
                    let exts: Vec<&str> = filter.extensions.iter().map(|s| s.as_str()).collect();
                    d = d.add_filter(&filter.name, &exts);
                }
            }
        }
        let handles = d.pick_files().await;
        handles
            .into_iter()
            .flatten()
            .map(|h| h.path().to_string_lossy().into_owned())
            .collect()
    }

    pub async fn pick_folder(options: Option<PickFolderOptions>) -> Option<String> {
        let mut d = rfd::AsyncFileDialog::new();
        if let Some(opts) = options {
            if let Some(title) = opts.title {
                d = d.set_title(&title);
            }
            if let Some(def_path) = opts.default_path {
                d = d.set_directory(&def_path);
            }
        }
        let handle = d.pick_folder().await?;
        Some(handle.path().to_string_lossy().into_owned())
    }

    pub async fn save_file(options: Option<SaveFileOptions>) -> Option<String> {
        let mut d = rfd::AsyncFileDialog::new();
        if let Some(opts) = options {
            if let Some(title) = opts.title {
                d = d.set_title(&title);
            }
            if let Some(def_path) = opts.default_path {
                d = d.set_file_name(&def_path);
            }
            if let Some(filters) = opts.filters {
                for filter in filters {
                    let exts: Vec<&str> = filter.extensions.iter().map(|s| s.as_str()).collect();
                    d = d.add_filter(&filter.name, &exts);
                }
            }
        }
        let handle = d.save_file().await?;
        Some(handle.path().to_string_lossy().into_owned())
    }

    pub async fn message(options: MessageOptions) {
        let level = match options.kind.as_deref() {
            Some("warning") => rfd::MessageLevel::Warning,
            Some("error") => rfd::MessageLevel::Error,
            _ => rfd::MessageLevel::Info,
        };
        let mut d = rfd::AsyncMessageDialog::new()
            .set_level(level)
            .set_description(&options.message);
        if let Some(title) = options.title {
            d = d.set_title(&title);
        }
        d.show().await;
    }
}

pub mod clipboard {
    use std::sync::Mutex;

    static CLIPBOARD_LOCK: Mutex<()> = Mutex::new(());

    pub fn read_text() -> Result<String, String> {
        let _guard = CLIPBOARD_LOCK.lock().unwrap();
        let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        cb.get_text().map_err(|e| e.to_string())
    }

    pub fn write_text(text: &str) -> Result<(), String> {
        let _guard = CLIPBOARD_LOCK.lock().unwrap();
        let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        cb.set_text(text).map_err(|e| e.to_string())
    }

    pub fn clear() -> Result<(), String> {
        let _guard = CLIPBOARD_LOCK.lock().unwrap();
        let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        cb.clear().map_err(|e| e.to_string())
    }
}

pub mod theme {
    pub fn get_theme() -> &'static str {
        match dark_light::detect() {
            dark_light::Mode::Dark => "dark",
            dark_light::Mode::Light => "light",
            dark_light::Mode::Default => "light",
        }
    }
}

pub mod notification {
    use super::*;

    pub async fn show(options: NotificationOptions) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            let connection = zbus::Connection::session()
                .await
                .map_err(|e| e.to_string())?;
            let app_name = options.app_name.as_deref().unwrap_or("Kiln");
            let body = options.body.as_deref().unwrap_or("");
            let empty_actions: &[&str] = &[];
            let empty_hints = std::collections::HashMap::<&str, zbus::zvariant::Value>::new();
            connection
                .call_method(
                    Some("org.freedesktop.Notifications"),
                    "/org/freedesktop/Notifications",
                    Some("org.freedesktop.Notifications"),
                    "Notify",
                    &(
                        app_name,
                        0u32,
                        "",
                        &options.title,
                        body,
                        empty_actions,
                        empty_hints,
                        -1i32,
                    ),
                )
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = options;
            Ok(())
        }
    }
}

pub mod tray {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tray_icon::{TrayIcon, TrayIconBuilder};

    static TRAY_ID_COUNTER: AtomicU32 = AtomicU32::new(1);
    thread_local! {
        static ACTIVE_TRAYS: RefCell<Vec<(u32, TrayIcon)>> = const { RefCell::new(Vec::new()) };
    }

    pub fn create_tray_icon(tooltip: &str) -> Result<u32, String> {
        let id = TRAY_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tray = TrayIconBuilder::new()
            .with_tooltip(tooltip)
            .build()
            .map_err(|e| e.to_string())?;

        ACTIVE_TRAYS.with(|trays| {
            trays.borrow_mut().push((id, tray));
        });
        Ok(id)
    }

    pub fn set_tooltip(id: u32, tooltip: &str) -> Result<(), String> {
        ACTIVE_TRAYS.with(|trays| {
            if let Some((_, tray)) = trays.borrow_mut().iter_mut().find(|(t_id, _)| *t_id == id) {
                tray.set_tooltip(Some(tooltip)).map_err(|e| e.to_string())
            } else {
                Err(format!("tray {id} not found"))
            }
        })
    }

    pub fn destroy_tray_icon(id: u32) {
        ACTIVE_TRAYS.with(|trays| {
            trays.borrow_mut().retain(|(t_id, _)| *t_id != id);
        });
    }

    pub fn clear_tray_icons() {
        ACTIVE_TRAYS.with(|trays| {
            trays.borrow_mut().clear();
        });
    }
}

pub mod menu {
    pub fn create_menu() -> muda::Menu {
        muda::Menu::new()
    }

    pub fn create_separator() -> muda::PredefinedMenuItem {
        muda::PredefinedMenuItem::separator()
    }

    pub fn create_submenu(text: &str, enabled: bool) -> muda::Submenu {
        muda::Submenu::new(text, enabled)
    }
}
