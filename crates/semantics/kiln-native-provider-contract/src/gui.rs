//! The host GUI bridge's half of the provider contract (ADR 0074).
//!
//! `kiln:gui` reaches the platform through the `kiln_gui_*` operations `kiln-host-gui` exports.
//! Those used to live in a `cdylib` the program `dlopen`ed from `target/` at run time, which made a
//! GUI binary work only inside a checkout that had built it. They are now linked into the provider
//! archive of every variant that declares the `gui` capability, and the native `node:ffi` engine
//! answers `dlopen(LIBRARY_SPECIFIER)` from the table the archive carries instead of from a file.
//!
//! This list is the packaged promise: a `gui` or `full-gui` archive must define every name here,
//! and the packager proves it against the archive's symbol table before it writes a manifest.
//! `kiln-host-gui` keeps the same list beside its definitions (`kiln_host_gui::ffi::exports`), and
//! each crate's tests hold the two to the source file so neither can drift.

/// The `node:ffi` library specifier that names the linked-in GUI bridge. It is not a path.
pub const LIBRARY_SPECIFIER: &str = "kiln:host-gui";

/// Every C-ABI operation the GUI bridge exports, in a stable order.
pub const GUI_EXPORTS: &[&str] = &[
    "kiln_gui_abi_version",
    "kiln_gui_result",
    "kiln_gui_init",
    "kiln_gui_create",
    "kiln_gui_set_text",
    "kiln_gui_set_number",
    "kiln_gui_insert",
    "kiln_gui_remove",
    "kiln_gui_destroy",
    "kiln_gui_call",
    "kiln_gui_wait",
    "kiln_gui_next_event",
    "kiln_gui_dialog",
    "kiln_gui_clipboard_read",
    "kiln_gui_clipboard_write",
    "kiln_gui_clipboard_clear",
    "kiln_gui_is_dark",
    "kiln_gui_notify",
    "kiln_gui_app_path",
    "kiln_gui_single_instance",
    "kiln_gui_set_menu",
    "kiln_gui_tray_create",
    "kiln_gui_tray_update",
    "kiln_gui_tray_destroy",
    "kiln_gui_hotkey_register",
    "kiln_gui_hotkey_unregister",
    "kiln_gui_describe",
];

/// Linker arguments a program needs when its provider archive carries the bridge: the OS
/// frameworks and runtime libraries `kiln-host-gui` and its dependencies declare through
/// `#[link]`. Kept here, per target, rather than read back from `rustc --print
/// native-static-libs`, because that note is printed only by a compiler invocation that actually
/// runs -- a cached or fresh rebuild says nothing -- and a list a reviewer can read is the one the
/// packager proves against the archive (`scripts/package-native-number-provider.sh` links a probe
/// that references every export). Empty for every target whose bridge brings no platform library.
pub fn link_arguments(rust_target: &str) -> &'static [&'static str] {
    if rust_target.contains("apple-darwin") {
        &[
            "-framework",
            "Cocoa",
            "-framework",
            "AppKit",
            "-framework",
            "Foundation",
            "-framework",
            "CoreFoundation",
            "-framework",
            "CoreGraphics",
            "-framework",
            "Carbon",
            "-lobjc",
            "-liconv",
        ]
    } else {
        &[]
    }
}

/// The `kiln_gui_*` names a source file defines with `pub extern "C"`, for the drift tests.
pub fn exported_names_in_source(source: &str) -> Vec<String> {
    let mut names = Vec::new();
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
        if name.starts_with("kiln_gui_") && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_list_matches_the_bridge_source() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("repository root");
        let source = std::fs::read_to_string(root.join("crates/host/kiln-host-gui/src/ffi.rs"))
            .expect("read kiln-host-gui ffi.rs");
        let mut defined = exported_names_in_source(&source);
        defined.sort();
        let mut listed: Vec<String> = GUI_EXPORTS.iter().map(|s| (*s).to_owned()).collect();
        listed.sort();
        assert_eq!(
            listed, defined,
            "GUI_EXPORTS must name exactly the kiln_gui_* functions kiln-host-gui defines"
        );
        let mut unique = GUI_EXPORTS.to_vec();
        unique.dedup();
        assert_eq!(
            unique.len(),
            GUI_EXPORTS.len(),
            "GUI_EXPORTS lists a name twice"
        );
    }

    #[test]
    fn link_arguments_name_frameworks_only_where_the_bridge_links_them() {
        assert!(link_arguments("aarch64-apple-darwin").contains(&"Cocoa"));
        assert!(link_arguments("x86_64-apple-darwin").contains(&"-lobjc"));
        assert!(link_arguments("x86_64-unknown-linux-gnu").is_empty());
        assert!(link_arguments("aarch64-unknown-linux-musl").is_empty());
    }

    #[test]
    fn specifier_is_not_a_path() {
        assert!(LIBRARY_SPECIFIER.starts_with("kiln:"));
        assert!(!LIBRARY_SPECIFIER.contains('/'));
    }
}
