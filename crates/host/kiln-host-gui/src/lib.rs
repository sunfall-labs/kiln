//! Host GUI and OS utility capability engine for Kiln (ADR 0074).
//! Provides cross-platform desktop dialogs, clipboard, system theme detection,
//! native menus, and system tray integration.

use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
pub mod appkit;
pub mod ffi;
pub mod layout;

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

pub mod app {
    use std::path::PathBuf;

    pub fn get_path(name: &str) -> Option<PathBuf> {
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
    #[cfg(unix)]
    use std::os::unix::net::UnixListener;
    use std::sync::Mutex;

    static LOCK_HOLDER: Mutex<Option<Box<dyn std::any::Any + Send>>> = Mutex::new(None);

    pub fn request_single_instance_lock(app_id: &str) -> bool {
        let mut guard = LOCK_HOLDER.lock().unwrap();
        if guard.is_some() {
            return true;
        }

        #[cfg(unix)]
        {
            let safe_id: String = app_id
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let sock_path = std::env::temp_dir().join(format!("kiln-app-{safe_id}.sock"));

            if std::os::unix::net::UnixStream::connect(&sock_path).is_ok() {
                return false;
            }
            let _ = std::fs::remove_file(&sock_path);

            match UnixListener::bind(&sock_path) {
                Ok(listener) => {
                    *guard = Some(Box::new(listener));
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
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, hotkey::HotKey};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::str::FromStr;

    thread_local! {
        static MANAGER: RefCell<Option<GlobalHotKeyManager>> = const { RefCell::new(None) };
        static HOTKEYS: RefCell<HashMap<u32, HotKey>> = RefCell::new(HashMap::new());
    }

    pub fn register(accelerator: &str) -> Result<u32, String> {
        let hotkey = HotKey::from_str(accelerator).map_err(|e| e.to_string())?;
        let id = hotkey.id();
        MANAGER.with(|mgr| {
            let mut mgr_borrow = mgr.borrow_mut();
            if mgr_borrow.is_none() {
                *mgr_borrow = Some(GlobalHotKeyManager::new().map_err(|e| e.to_string())?);
            }
            if let Some(m) = mgr_borrow.as_ref() {
                m.register(hotkey).map_err(|e| e.to_string())?;
            }
            HOTKEYS.with(|keys| {
                keys.borrow_mut().insert(id, hotkey);
            });
            Ok(id)
        })
    }

    pub fn unregister(id: u32) -> Result<(), String> {
        MANAGER.with(|mgr| {
            if let Some(m) = mgr.borrow().as_ref() {
                HOTKEYS.with(|keys| {
                    if let Some(hotkey) = keys.borrow_mut().remove(&id) {
                        let _ = m.unregister(hotkey);
                    }
                });
            }
            Ok(())
        })
    }

    pub fn poll_event() -> Option<u32> {
        if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            Some(event.id())
        } else {
            None
        }
    }
}
