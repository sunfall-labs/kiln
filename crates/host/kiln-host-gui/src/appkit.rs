#![cfg(target_os = "macos")]
#![allow(unexpected_cfgs)]
#![allow(
    clippy::missing_safety_doc,
    clippy::collapsible_if,
    clippy::collapsible_match
)]

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;
use std::sync::Mutex;

use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

const NO: i8 = 0;
const YES: i8 = 1;

#[link(name = "Cocoa", kind = "framework")]
unsafe extern "C" {}

#[derive(Copy, Clone)]
pub struct SendPtr<T>(pub *mut T);
unsafe impl<T> Send for SendPtr<T> {}
unsafe impl<T> Sync for SendPtr<T> {}

impl<T> std::ops::Deref for SendPtr<T> {
    type Target = *mut T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct NSEdgeInsets {
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
}

const NS_BACKING_STORE_BUFFERED: usize = 2;

static INITIALIZED: Mutex<bool> = Mutex::new(false);
static ACTION_TARGET: Mutex<SendPtr<Object>> = Mutex::new(SendPtr(ptr::null_mut()));
static PENDING_EVENTS: Mutex<Vec<(u64, String, String)>> = Mutex::new(Vec::new());

pub struct WindowEntry {
    pub window: SendPtr<Object>,
    pub root_view: SendPtr<Object>,
    pub was_visible: bool,
}

pub struct WidgetEntry {
    pub view: SendPtr<Object>,
    pub tag: String,
}

static WINDOWS: Mutex<Option<HashMap<u64, WindowEntry>>> = Mutex::new(None);
static WIDGETS: Mutex<Option<HashMap<u64, WidgetEntry>>> = Mutex::new(None);

#[allow(clippy::collapsible_if)]
extern "C" fn button_action(_this: &Object, _cmd: Sel, sender: *mut Object) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        if tag > 0 {
            if let Ok(mut events) = PENDING_EVENTS.lock() {
                events.push((tag as u64, "click".to_string(), String::new()));
            }
        }
    }
}

extern "C" fn slider_action(_this: &Object, _cmd: Sel, sender: *mut Object) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        let val: f64 = msg_send![sender, doubleValue];
        if tag > 0 {
            if let Ok(mut events) = PENDING_EVENTS.lock() {
                events.push((tag as u64, "change".to_string(), val.to_string()));
            }
        }
    }
}

extern "C" fn switch_action(_this: &Object, _cmd: Sel, sender: *mut Object) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        let state: isize = msg_send![sender, state];
        if tag > 0 {
            if let Ok(mut events) = PENDING_EVENTS.lock() {
                events.push((
                    tag as u64,
                    "toggle".to_string(),
                    if state == 1 {
                        "true".to_string()
                    } else {
                        "false".to_string()
                    },
                ));
            }
        }
    }
}

extern "C" fn control_text_did_change(_this: &Object, _cmd: Sel, notification: *mut Object) {
    unsafe {
        let text_field: *mut Object = msg_send![notification, object];
        let tag: isize = msg_send![text_field, tag];
        let str_obj: *mut Object = msg_send![text_field, stringValue];
        let utf8: *const c_char = msg_send![str_obj, UTF8String];
        let text = if utf8.is_null() {
            String::new()
        } else {
            CStr::from_ptr(utf8).to_string_lossy().into_owned()
        };
        if tag > 0 {
            if let Ok(mut events) = PENDING_EVENTS.lock() {
                events.push((tag as u64, "input".to_string(), text));
            }
        }
    }
}

extern "C" fn window_will_close(_this: &Object, _cmd: Sel, notification: *mut Object) {
    unsafe {
        let win: *mut Object = msg_send![notification, object];
        let mut target_id = None;
        if let Ok(wins) = WINDOWS.lock() {
            if let Some(map) = wins.as_ref() {
                for (&id, entry) in map.iter() {
                    if entry.window.0 == win {
                        target_id = Some(id);
                        break;
                    }
                }
            }
        }
        if let Some(id) = target_id {
            if let Ok(mut events) = PENDING_EVENTS.lock() {
                events.push((id, "close".to_string(), String::new()));
            }
        }
    }
}

extern "C" fn segmented_action(_this: &Object, _cmd: Sel, sender: *mut Object) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        let seg: isize = msg_send![sender, selectedSegment];
        if tag > 0 {
            if let Ok(mut events) = PENDING_EVENTS.lock() {
                events.push((tag as u64, "change".to_string(), seg.to_string()));
            }
        }
    }
}

extern "C" fn select_action(_this: &Object, _cmd: Sel, sender: *mut Object) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        let idx: isize = msg_send![sender, indexOfSelectedItem];
        let title_obj: *mut Object = msg_send![sender, titleOfSelectedItem];
        let utf8: *const c_char = if !title_obj.is_null() {
            msg_send![title_obj, UTF8String]
        } else {
            ptr::null()
        };
        let title = if utf8.is_null() {
            String::new()
        } else {
            CStr::from_ptr(utf8).to_string_lossy().into_owned()
        };
        if tag > 0 {
            if let Ok(mut events) = PENDING_EVENTS.lock() {
                events.push((
                    tag as u64,
                    "change".to_string(),
                    format!("{}:{}", idx, title),
                ));
            }
        }
    }
}

fn parse_items_json_or_csv(s: &str) -> Vec<String> {
    if s.starts_with('[') {
        serde_json::from_str::<Vec<String>>(s)
            .unwrap_or_else(|_| s.split(',').map(|item| item.trim().to_string()).collect())
    } else {
        s.split(',').map(|item| item.trim().to_string()).collect()
    }
}

fn setup_segmented_items(seg: *mut Object, s: &str) {
    let items = parse_items_json_or_csv(s);
    if !items.is_empty() {
        unsafe {
            let count = items.len() as isize;
            let () = msg_send![seg, setSegmentCount: count];
            for (i, label) in items.iter().enumerate() {
                let label_obj = ns_string(label);
                let () = msg_send![seg, setLabel: label_obj forSegment: i as isize];
            }
            let () = msg_send![seg, setSelectedSegment: 0isize];
        }
    }
}

fn setup_popup_items(popup: *mut Object, s: &str) {
    let items = parse_items_json_or_csv(s);
    unsafe {
        let () = msg_send![popup, removeAllItems];
        for item in items {
            let label_obj = ns_string(&item);
            let () = msg_send![popup, addItemWithTitle: label_obj];
        }
    }
}

fn set_image_from_symbol_or_path(img_view: *mut Object, name: &str) {
    let symbol_name = match name {
        "settings" => "gearshape",
        "search" => "magnifyingglass",
        "play" => "play.fill",
        "pause" => "pause.fill",
        "stop" => "stop.fill",
        "refresh" => "arrow.clockwise",
        "terminal" => "terminal.fill",
        "cpu" => "cpu",
        "memory" => "memorychip",
        "check" | "checkmark" | "success" => "checkmark.circle.fill",
        "error" | "danger" => "exclamationmark.triangle.fill",
        "info" => "info.circle.fill",
        "folder" => "folder.fill",
        "file" => "doc.text.fill",
        other => other,
    };
    unsafe {
        let img_cls = class!(NSImage);
        let sym_ns = ns_string(symbol_name);
        let img: *mut Object = msg_send![
            img_cls,
            imageWithSystemSymbolName: sym_ns
            accessibilityDescription: ptr::null_mut::<Object>()
        ];
        if !img.is_null() {
            let () = msg_send![img_view, setImage: img];
        }
    }
}

fn get_or_create_action_target() -> *mut Object {
    let mut guard = ACTION_TARGET.lock().unwrap();
    if !guard.0.is_null() {
        return guard.0;
    }

    let target_class = if let Some(cls) = Class::get("KilnAppKitActionTarget") {
        cls
    } else {
        let superclass = class!(NSObject);
        let mut decl = ClassDecl::new("KilnAppKitActionTarget", superclass).unwrap();
        unsafe {
            decl.add_method(
                sel!(buttonAction:),
                button_action as extern "C" fn(&Object, Sel, *mut Object),
            );
            decl.add_method(
                sel!(sliderAction:),
                slider_action as extern "C" fn(&Object, Sel, *mut Object),
            );
            decl.add_method(
                sel!(switchAction:),
                switch_action as extern "C" fn(&Object, Sel, *mut Object),
            );
            decl.add_method(
                sel!(controlTextDidChange:),
                control_text_did_change as extern "C" fn(&Object, Sel, *mut Object),
            );
            decl.add_method(
                sel!(windowWillClose:),
                window_will_close as extern "C" fn(&Object, Sel, *mut Object),
            );
            decl.add_method(
                sel!(segmentedAction:),
                segmented_action as extern "C" fn(&Object, Sel, *mut Object),
            );
            decl.add_method(
                sel!(selectAction:),
                select_action as extern "C" fn(&Object, Sel, *mut Object),
            );
        }
        decl.register()
    };

    let target: *mut Object = unsafe {
        let alloc: *mut Object = msg_send![target_class, alloc];
        msg_send![alloc, init]
    };
    guard.0 = target;
    target
}

pub fn ensure_initialized() -> bool {
    let mut guard = INITIALIZED.lock().unwrap();
    if *guard {
        return true;
    }
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return false;
        }
        // NSApplicationActivationPolicyRegular = 0
        let () = msg_send![app, setActivationPolicy: 0isize];
        let () = msg_send![app, finishLaunching];
    }
    let mut wins = WINDOWS.lock().unwrap();
    if wins.is_none() {
        *wins = Some(HashMap::new());
    }
    let mut wids = WIDGETS.lock().unwrap();
    if wids.is_none() {
        *wids = Some(HashMap::new());
    }
    *guard = true;
    true
}

unsafe fn ns_string(s: &str) -> *mut Object {
    let c = CString::new(s).unwrap_or_default();
    msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()]
}

pub fn window_create(id: u64, title: &str, width: f64, height: f64) -> bool {
    if !ensure_initialized() {
        return false;
    }
    unsafe {
        let rect = CGRect {
            origin: CGPoint { x: 200.0, y: 200.0 },
            size: CGSize {
                width: if width > 0.0 { width } else { 600.0 },
                height: if height > 0.0 { height } else { 400.0 },
            },
        };

        let alloc: *mut Object = msg_send![class!(NSWindow), alloc];
        const MODERN_STYLE_MASK: usize = 1 | 2 | 4 | 8 | 32768;
        let win: *mut Object = msg_send![
            alloc,
            initWithContentRect: rect
            styleMask: MODERN_STYLE_MASK
            backing: NS_BACKING_STORE_BUFFERED
            defer: NO
        ];

        let title_obj = ns_string(title);
        let () = msg_send![win, setTitle: title_obj];
        let () = msg_send![win, center];
        let () = msg_send![win, setReleasedWhenClosed: NO];
        let target = get_or_create_action_target();
        let () = msg_send![win, setDelegate: target];

        let () = msg_send![win, setTitlebarAppearsTransparent: YES];

        // Create an NSVisualEffectView for vibrant, translucent macOS background
        let effect_alloc: *mut Object = msg_send![class!(NSVisualEffectView), alloc];
        let effect: *mut Object = msg_send![effect_alloc, initWithFrame: rect];
        // NSVisualEffectMaterialUnderWindowBackground = 2
        let () = msg_send![effect, setMaterial: 2isize];
        // NSVisualEffectBlendingModeBehindWindow = 0
        let () = msg_send![effect, setBlendingMode: 0isize];
        // NSVisualEffectStateActive = 1
        let () = msg_send![effect, setState: 1isize];
        let () = msg_send![effect, setWantsLayer: YES];
        // NSViewWidthSizable (2) | NSViewHeightSizable (16) = 18
        let () = msg_send![effect, setAutoresizingMask: 18usize];

        // Create a root vertical NSStackView as the default content view
        let stack_alloc: *mut Object = msg_send![class!(NSStackView), alloc];
        let stack: *mut Object = msg_send![stack_alloc, initWithFrame: rect];
        let () = msg_send![stack, setOrientation: 1isize];
        let () = msg_send![stack, setSpacing: 16.0f64];
        let insets = NSEdgeInsets {
            top: 56.0,
            left: 36.0,
            bottom: 28.0,
            right: 36.0,
        };
        let () = msg_send![stack, setEdgeInsets: insets];
        let () = msg_send![stack, setAlignment: 5isize]; // NSLayoutAttributeLeading
        let () = msg_send![stack, setWantsLayer: YES];
        let () = msg_send![stack, setAutoresizingMask: 18usize];

        let () = msg_send![effect, addSubview: stack];
        let () = msg_send![win, setContentView: effect];

        let mut wins = WINDOWS.lock().unwrap();
        if let Some(map) = wins.as_mut() {
            map.insert(
                id,
                WindowEntry {
                    window: SendPtr(win),
                    root_view: SendPtr(effect),
                    was_visible: false,
                },
            );
        }
    }
    true
}

pub fn window_show(id: u64) {
    if !ensure_initialized() {
        return;
    }
    let win_ptr = {
        let mut wins = WINDOWS.lock().unwrap();
        if let Some(entry) = wins.as_mut().and_then(|m| m.get_mut(&id)) {
            entry.was_visible = true;
            Some(entry.window.0)
        } else {
            None
        }
    };
    if let Some(win) = win_ptr {
        unsafe {
            let () = msg_send![win, makeKeyAndOrderFront: ptr::null_mut::<Object>()];
            let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
            let () = msg_send![app, activateIgnoringOtherApps: YES];
        }
    }
}

pub fn window_close(id: u64) {
    let win_ptr = {
        let mut wins = WINDOWS.lock().unwrap();
        wins.as_mut()
            .and_then(|m| m.remove(&id))
            .map(|e| e.window.0)
    };
    if let Some(win) = win_ptr {
        unsafe {
            let () = msg_send![win, close];
        }
    }
}

pub fn window_set_title(id: u64, title: &str) {
    let win_ptr = {
        let wins = WINDOWS.lock().unwrap();
        wins.as_ref().and_then(|m| m.get(&id)).map(|e| e.window.0)
    };
    if let Some(win) = win_ptr {
        unsafe {
            let title_obj = ns_string(title);
            let () = msg_send![win, setTitle: title_obj];
        }
    }
}

pub fn window_set_content(win_id: u64, child_id: u64) {
    let (win_ptr, root_effect_ptr) = {
        let wins = WINDOWS.lock().unwrap();
        if let Some(entry) = wins.as_ref().and_then(|m| m.get(&win_id)) {
            (Some(entry.window.0), Some(entry.root_view.0))
        } else {
            (None, None)
        }
    };
    let view_ptr = {
        let wids = WIDGETS.lock().unwrap();
        wids.as_ref()
            .and_then(|m| m.get(&child_id))
            .map(|e| e.view.0)
    };
    if let (Some(win), Some(view)) = (win_ptr, view_ptr) {
        unsafe {
            let () = msg_send![view, setWantsLayer: YES];
            let () = msg_send![view, setAutoresizingMask: 18usize];
            if let Some(effect) = root_effect_ptr {
                let bounds: CGRect = msg_send![effect, bounds];
                let () = msg_send![view, setFrame: bounds];

                let is_stack: bool = msg_send![view, isKindOfClass: class!(NSStackView)];
                if is_stack {
                    let insets = NSEdgeInsets {
                        top: 56.0,
                        left: 36.0,
                        bottom: 28.0,
                        right: 36.0,
                    };
                    let () = msg_send![view, setEdgeInsets: insets];
                    let () = msg_send![view, setAlignment: 5isize]; // NSLayoutAttributeLeading
                }

                let subviews: *mut Object = msg_send![effect, subviews];
                let count: usize = msg_send![subviews, count];
                for i in (0..count).rev() {
                    let sub: *mut Object = msg_send![subviews, objectAtIndex: i];
                    let () = msg_send![sub, removeFromSuperview];
                }
                let () = msg_send![effect, addSubview: view];
            } else {
                let () = msg_send![win, setContentView: view];
            }
        }
    }
}

pub fn widget_create(id: u64, tag: &str, text: &str) -> bool {
    if !ensure_initialized() {
        return false;
    }
    unsafe {
        let target = get_or_create_action_target();
        let view: *mut Object = match tag {
            "vstack" => {
                let alloc: *mut Object = msg_send![class!(NSStackView), alloc];
                let rect = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 400.0,
                        height: 300.0,
                    },
                };
                let stack: *mut Object = msg_send![alloc, initWithFrame: rect];
                // Orientation vertical = 1
                let () = msg_send![stack, setOrientation: 1isize];
                let () = msg_send![stack, setSpacing: 12.0f64];
                let () = msg_send![stack, setAlignment: 5isize]; // NSLayoutAttributeLeading
                let () = msg_send![stack, setWantsLayer: YES];
                stack
            }
            "hstack" => {
                let alloc: *mut Object = msg_send![class!(NSStackView), alloc];
                let rect = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 400.0,
                        height: 36.0,
                    },
                };
                let stack: *mut Object = msg_send![alloc, initWithFrame: rect];
                // Orientation horizontal = 0
                let () = msg_send![stack, setOrientation: 0isize];
                let () = msg_send![stack, setSpacing: 12.0f64];
                let () = msg_send![stack, setAlignment: 10isize]; // NSLayoutAttributeCenterY
                let () = msg_send![stack, setWantsLayer: YES];
                stack
            }
            "card" | "box" => {
                let alloc: *mut Object = msg_send![class!(NSBox), alloc];
                let rect = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 520.0,
                        height: 80.0,
                    },
                };
                let box_view: *mut Object = msg_send![alloc, initWithFrame: rect];
                // NSBoxCustom = 4
                let () = msg_send![box_view, setBoxType: 4isize];
                let () = msg_send![box_view, setCornerRadius: 10.0f64];
                let () = msg_send![box_view, setBorderWidth: 1.0f64];
                let color_cls = class!(NSColor);
                let border_color: *mut Object = msg_send![color_cls, separatorColor];
                let () = msg_send![box_view, setBorderColor: border_color];
                let fill_color: *mut Object =
                    msg_send![color_cls, colorWithWhite: 1.0f64 alpha: 0.05f64];
                let () = msg_send![box_view, setFillColor: fill_color];
                let margins = CGSize {
                    width: 16.0,
                    height: 14.0,
                };
                let () = msg_send![box_view, setContentViewMargins: margins];
                let () = msg_send![box_view, setWantsLayer: YES];
                let width_anchor: *mut Object = msg_send![box_view, widthAnchor];
                let constraint: *mut Object =
                    msg_send![width_anchor, constraintGreaterThanOrEqualToConstant: 480.0f64];
                let () = msg_send![constraint, setActive: YES];
                box_view
            }
            "divider" | "separator" => {
                let alloc: *mut Object = msg_send![class!(NSBox), alloc];
                let rect = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 500.0,
                        height: 1.0,
                    },
                };
                let box_view: *mut Object = msg_send![alloc, initWithFrame: rect];
                // NSBoxSeparator = 2
                let () = msg_send![box_view, setBoxType: 2isize];
                let () = msg_send![box_view, setWantsLayer: YES];
                let width_anchor: *mut Object = msg_send![box_view, widthAnchor];
                let constraint: *mut Object =
                    msg_send![width_anchor, constraintGreaterThanOrEqualToConstant: 480.0f64];
                let () = msg_send![constraint, setActive: YES];
                box_view
            }
            "button" => {
                let title_obj = ns_string(text);
                let btn: *mut Object = msg_send![
                    class!(NSButton),
                    buttonWithTitle: title_obj
                    target: target
                    action: sel!(buttonAction:)
                ];
                // NSRoundedBezelStyle = 1
                let () = msg_send![btn, setBezelStyle: 1isize];
                let () = msg_send![btn, setTag: id as isize];
                btn
            }
            "text" => {
                let text_obj = ns_string(text);
                let label: *mut Object = msg_send![class!(NSTextField), labelWithString: text_obj];
                let font: *mut Object = msg_send![class!(NSFont), systemFontOfSize: 13.0f64];
                let () = msg_send![label, setFont: font];
                let () = msg_send![label, setWantsLayer: YES];
                label
            }
            "textinput" => {
                let text_obj = ns_string(text);
                let field: *mut Object =
                    msg_send![class!(NSTextField), textFieldWithString: text_obj];
                let () = msg_send![field, setEditable: YES];
                let () = msg_send![field, setSelectable: YES];
                let () = msg_send![field, setDelegate: target];
                let () = msg_send![field, setTag: id as isize];
                let font: *mut Object = msg_send![class!(NSFont), systemFontOfSize: 13.0f64];
                let () = msg_send![field, setFont: font];
                let () = msg_send![field, setBezelStyle: 1isize]; // NSTextFieldRoundedBezel = 1
                let () = msg_send![field, setWantsLayer: YES];
                let width_anchor: *mut Object = msg_send![field, widthAnchor];
                let constraint: *mut Object =
                    msg_send![width_anchor, constraintGreaterThanOrEqualToConstant: 240.0f64];
                let () = msg_send![constraint, setActive: YES];
                field
            }
            "slider" => {
                let alloc: *mut Object = msg_send![class!(NSSlider), alloc];
                let slider: *mut Object = msg_send![
                    alloc,
                    initWithFrame: CGRect {
                        origin: CGPoint { x: 0.0, y: 0.0 },
                        size: CGSize {
                            width: 220.0,
                            height: 24.0,
                        },
                    }
                ];
                let () = msg_send![slider, setTarget: target];
                let () = msg_send![slider, setAction: sel!(sliderAction:)];
                let () = msg_send![slider, setTag: id as isize];
                let () = msg_send![slider, setContinuous: YES];
                let () = msg_send![slider, setMinValue: 0.0f64];
                let () = msg_send![slider, setMaxValue: 100.0f64];
                let () = msg_send![slider, setDoubleValue: 0.0f64];
                let () = msg_send![slider, setWantsLayer: YES];
                let width_anchor: *mut Object = msg_send![slider, widthAnchor];
                let constraint: *mut Object =
                    msg_send![width_anchor, constraintGreaterThanOrEqualToConstant: 200.0f64];
                let () = msg_send![constraint, setActive: YES];
                slider
            }
            "toggle" => {
                let sw: *mut Object = if let Some(cls) = Class::get("NSSwitch") {
                    let alloc: *mut Object = msg_send![cls, alloc];
                    let s: *mut Object = msg_send![alloc, init];
                    let () = msg_send![s, setTarget: target];
                    let () = msg_send![s, setAction: sel!(switchAction:)];
                    let () = msg_send![s, setTag: id as isize];
                    let () = msg_send![s, setWantsLayer: YES];
                    s
                } else {
                    let btn: *mut Object = msg_send![
                        class!(NSButton),
                        checkboxWithTitle: ns_string(text)
                        target: target
                        action: sel!(switchAction:)
                    ];
                    let () = msg_send![btn, setTag: id as isize];
                    let () = msg_send![btn, setWantsLayer: YES];
                    btn
                };
                sw
            }
            "spacer" => {
                let alloc: *mut Object = msg_send![class!(NSView), alloc];
                let rect = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 0.0,
                        height: 0.0,
                    },
                };
                let view: *mut Object = msg_send![alloc, initWithFrame: rect];
                let () = msg_send![view, setTranslatesAutoresizingMaskIntoConstraints: NO];
                let () = msg_send![view, setContentHuggingPriority: 1.0f32 forOrientation: 0isize];
                let () = msg_send![view, setContentHuggingPriority: 1.0f32 forOrientation: 1isize];
                let () = msg_send![
                    view,
                    setContentCompressionResistancePriority: 1.0f32 forOrientation: 0isize
                ];
                let () = msg_send![
                    view,
                    setContentCompressionResistancePriority: 1.0f32 forOrientation: 1isize
                ];
                view
            }
            "scroll" | "scrollview" => {
                let alloc: *mut Object = msg_send![class!(NSScrollView), alloc];
                let rect = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 400.0,
                        height: 250.0,
                    },
                };
                let scroll: *mut Object = msg_send![alloc, initWithFrame: rect];
                let () = msg_send![scroll, setHasVerticalScroller: YES];
                let () = msg_send![scroll, setHasHorizontalScroller: NO];
                let () = msg_send![scroll, setAutohidesScrollers: YES];
                let () = msg_send![scroll, setDrawsBackground: NO];
                let () = msg_send![scroll, setTranslatesAutoresizingMaskIntoConstraints: NO];
                scroll
            }
            "segmented" => {
                let alloc: *mut Object = msg_send![class!(NSSegmentedControl), alloc];
                let rect = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 260.0,
                        height: 28.0,
                    },
                };
                let seg: *mut Object = msg_send![alloc, initWithFrame: rect];
                let () = msg_send![seg, setSegmentStyle: 2isize]; // NSSegmentStyleRounded = 2
                let () = msg_send![seg, setTarget: target];
                let () = msg_send![seg, setAction: sel!(segmentedAction:)];
                let () = msg_send![seg, setTag: id as isize];
                let () = msg_send![seg, setWantsLayer: YES];
                if !text.is_empty() {
                    setup_segmented_items(seg, text);
                }
                seg
            }
            "progressbar" | "progress" => {
                let alloc: *mut Object = msg_send![class!(NSProgressIndicator), alloc];
                let rect = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 220.0,
                        height: 16.0,
                    },
                };
                let bar: *mut Object = msg_send![alloc, initWithFrame: rect];
                let () = msg_send![bar, setStyle: 0usize]; // NSProgressIndicatorBarStyle = 0
                let () = msg_send![bar, setIndeterminate: NO];
                let () = msg_send![bar, setMinValue: 0.0f64];
                let () = msg_send![bar, setMaxValue: 100.0f64];
                let () = msg_send![bar, setDoubleValue: 0.0f64];
                let () = msg_send![bar, setWantsLayer: YES];
                let width_anchor: *mut Object = msg_send![bar, widthAnchor];
                let constraint: *mut Object =
                    msg_send![width_anchor, constraintGreaterThanOrEqualToConstant: 160.0f64];
                let () = msg_send![constraint, setActive: YES];
                bar
            }
            "select" | "dropdown" => {
                let alloc: *mut Object = msg_send![class!(NSPopUpButton), alloc];
                let rect = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 180.0,
                        height: 26.0,
                    },
                };
                let popup: *mut Object = msg_send![alloc, initWithFrame: rect pullsDown: NO];
                let () = msg_send![popup, setTarget: target];
                let () = msg_send![popup, setAction: sel!(selectAction:)];
                let () = msg_send![popup, setTag: id as isize];
                let () = msg_send![popup, setWantsLayer: YES];
                let () = msg_send![popup, setBezelStyle: 1isize];
                if !text.is_empty() {
                    setup_popup_items(popup, text);
                }
                popup
            }
            "checkbox" => {
                let title_obj = ns_string(text);
                let btn: *mut Object = msg_send![
                    class!(NSButton),
                    checkboxWithTitle: title_obj
                    target: target
                    action: sel!(switchAction:)
                ];
                let () = msg_send![btn, setTag: id as isize];
                let () = msg_send![btn, setWantsLayer: YES];
                btn
            }
            "icon" | "image" => {
                let alloc: *mut Object = msg_send![class!(NSImageView), alloc];
                let rect = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 24.0,
                        height: 24.0,
                    },
                };
                let img_view: *mut Object = msg_send![alloc, initWithFrame: rect];
                let () = msg_send![img_view, setWantsLayer: YES];
                let () = msg_send![img_view, setImageScaling: 2usize]; // NSImageScaleProportionallyUpOrDown = 2
                if !text.is_empty() {
                    set_image_from_symbol_or_path(img_view, text);
                }
                img_view
            }
            _ => {
                let alloc: *mut Object = msg_send![class!(NSView), alloc];
                let rect = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 100.0,
                        height: 30.0,
                    },
                };
                let v: *mut Object = msg_send![alloc, initWithFrame: rect];
                let () = msg_send![v, setWantsLayer: YES];
                v
            }
        };

        let mut wids = WIDGETS.lock().unwrap();
        if let Some(map) = wids.as_mut() {
            map.insert(
                id,
                WidgetEntry {
                    view: SendPtr(view),
                    tag: tag.to_string(),
                },
            );
        }
    }
    true
}

pub fn widget_set_property(id: u64, name: &str, val_str: &str, val_num: f64) {
    let entry_info = {
        let wids = WIDGETS.lock().unwrap();
        wids.as_ref()
            .and_then(|m| m.get(&id))
            .map(|e| (e.tag.clone(), e.view.0))
    };
    if let Some((tag, view)) = entry_info {
        unsafe {
            match (tag.as_str(), name) {
                ("text", "text")
                | ("text", "value")
                | ("textinput", "value")
                | ("textinput", "text") => {
                    let ns = ns_string(val_str);
                    let () = msg_send![view, setStringValue: ns];
                }
                ("textinput", "placeholder") => {
                    let ns = ns_string(val_str);
                    let () = msg_send![view, setPlaceholderString: ns];
                }
                ("text", "variant") => {
                    let font_cls = class!(NSFont);
                    let color_cls = class!(NSColor);
                    match val_str {
                        "title" | "largeTitle" => {
                            let font: *mut Object =
                                msg_send![font_cls, boldSystemFontOfSize: 22.0f64];
                            let () = msg_send![view, setFont: font];
                            let color: *mut Object = msg_send![color_cls, labelColor];
                            let () = msg_send![view, setTextColor: color];
                        }
                        "headline" | "bold" => {
                            let font: *mut Object =
                                msg_send![font_cls, boldSystemFontOfSize: 15.0f64];
                            let () = msg_send![view, setFont: font];
                            let color: *mut Object = msg_send![color_cls, labelColor];
                            let () = msg_send![view, setTextColor: color];
                        }
                        "subheadline" | "subtitle" | "secondary" => {
                            let font: *mut Object = msg_send![font_cls, systemFontOfSize: 13.0f64];
                            let () = msg_send![view, setFont: font];
                            let color: *mut Object = msg_send![color_cls, secondaryLabelColor];
                            let () = msg_send![view, setTextColor: color];
                        }
                        "caption" | "tertiary" => {
                            let font: *mut Object = msg_send![font_cls, systemFontOfSize: 11.0f64];
                            let () = msg_send![view, setFont: font];
                            let color: *mut Object = msg_send![color_cls, tertiaryLabelColor];
                            let () = msg_send![view, setTextColor: color];
                        }
                        "badge" => {
                            let font: *mut Object =
                                msg_send![font_cls, boldSystemFontOfSize: 11.0f64];
                            let () = msg_send![view, setFont: font];
                            let color: *mut Object = msg_send![color_cls, secondaryLabelColor];
                            let () = msg_send![view, setTextColor: color];
                        }
                        _ => {}
                    }
                }
                ("text", "color") => {
                    let color_cls = class!(NSColor);
                    let color: *mut Object = match val_str {
                        "secondary" => msg_send![color_cls, secondaryLabelColor],
                        "tertiary" => msg_send![color_cls, tertiaryLabelColor],
                        "blue" | "accent" => msg_send![color_cls, systemBlueColor],
                        "green" | "success" => msg_send![color_cls, systemGreenColor],
                        "red" | "danger" => msg_send![color_cls, systemRedColor],
                        "orange" | "warning" => msg_send![color_cls, systemOrangeColor],
                        _ => msg_send![color_cls, labelColor],
                    };
                    let () = msg_send![view, setTextColor: color];
                }
                ("text", "size") => {
                    if val_num > 0.0 {
                        let font_cls = class!(NSFont);
                        let font: *mut Object = msg_send![font_cls, systemFontOfSize: val_num];
                        let () = msg_send![view, setFont: font];
                    }
                }
                ("button", "label") => {
                    let ns = ns_string(val_str);
                    let () = msg_send![view, setTitle: ns];
                }
                ("button", "variant") => {
                    let color_cls = class!(NSColor);
                    if val_str == "primary" {
                        let blue: *mut Object = msg_send![color_cls, systemBlueColor];
                        let () = msg_send![view, setBezelColor: blue];
                        let () = msg_send![view, setKeyEquivalent: ns_string("\r")];
                    } else if val_str == "danger" || val_str == "destructive" {
                        let red: *mut Object = msg_send![color_cls, systemRedColor];
                        let () = msg_send![view, setBezelColor: red];
                    }
                }
                ("button", "icon") => {
                    let sym_ns = ns_string(match val_str {
                        "settings" => "gearshape",
                        "search" => "magnifyingglass",
                        "play" => "play.fill",
                        "pause" => "pause.fill",
                        "stop" => "stop.fill",
                        "refresh" => "arrow.clockwise",
                        "terminal" => "terminal.fill",
                        other => other,
                    });
                    let img: *mut Object = msg_send![
                        class!(NSImage),
                        imageWithSystemSymbolName: sym_ns
                        accessibilityDescription: ptr::null_mut::<Object>()
                    ];
                    if !img.is_null() {
                        let () = msg_send![view, setImage: img];
                        let () = msg_send![view, setImagePosition: 2usize]; // NSImageLeft = 2
                    }
                }
                ("segmented", "items") => {
                    setup_segmented_items(view, val_str);
                }
                ("segmented", "selectedIndex") | ("segmented", "value") => {
                    let () = msg_send![view, setSelectedSegment: val_num as isize];
                }
                ("progressbar", "value") | ("progress", "value") => {
                    let () = msg_send![view, setDoubleValue: val_num];
                }
                ("progressbar", "max") | ("progress", "max") => {
                    let () = msg_send![view, setMaxValue: val_num];
                }
                ("progressbar", "min") | ("progress", "min") => {
                    let () = msg_send![view, setMinValue: val_num];
                }
                ("progressbar", "indeterminate") | ("progress", "indeterminate") => {
                    let is_indet = val_str == "true" || val_num > 0.0;
                    let () = msg_send![view, setIndeterminate: if is_indet { YES } else { NO }];
                    if is_indet {
                        let () = msg_send![view, startAnimation: ptr::null_mut::<Object>()];
                    } else {
                        let () = msg_send![view, stopAnimation: ptr::null_mut::<Object>()];
                    }
                }
                ("select", "options")
                | ("select", "items")
                | ("dropdown", "options")
                | ("dropdown", "items") => {
                    setup_popup_items(view, val_str);
                }
                ("select", "selectedIndex")
                | ("select", "value")
                | ("dropdown", "selectedIndex")
                | ("dropdown", "value") => {
                    let () = msg_send![view, selectItemAtIndex: val_num as isize];
                }
                ("checkbox", "checked") => {
                    let state = if val_str == "true" || val_num > 0.0 {
                        1isize
                    } else {
                        0isize
                    };
                    let () = msg_send![view, setState: state];
                }
                ("checkbox", "title") | ("checkbox", "label") | ("checkbox", "text") => {
                    let ns = ns_string(val_str);
                    let () = msg_send![view, setTitle: ns];
                }
                ("icon", "name")
                | ("icon", "symbol")
                | ("image", "name")
                | ("image", "symbol")
                | ("image", "src") => {
                    set_image_from_symbol_or_path(view, val_str);
                }
                ("slider", "value") => {
                    let () = msg_send![view, setDoubleValue: val_num];
                }
                ("slider", "min") => {
                    let () = msg_send![view, setMinValue: val_num];
                }
                ("slider", "max") => {
                    let () = msg_send![view, setMaxValue: val_num];
                }
                ("toggle", "checked") => {
                    let state = if val_str == "true" || val_num > 0.0 {
                        1isize
                    } else {
                        0isize
                    };
                    let () = msg_send![view, setState: state];
                }
                ("vstack", "spacing") | ("hstack", "spacing") => {
                    let () = msg_send![view, setSpacing: val_num];
                }
                ("vstack", "alignment") => {
                    let attr: isize = match val_str {
                        "leading" | "left" => 5,
                        "center" => 9,
                        "trailing" | "right" => 6,
                        _ => 5,
                    };
                    let () = msg_send![view, setAlignment: attr];
                }
                ("hstack", "alignment") => {
                    let attr: isize = match val_str {
                        "center" => 10,
                        "top" => 3,
                        "bottom" => 4,
                        "baseline" => 11,
                        _ => 10,
                    };
                    let () = msg_send![view, setAlignment: attr];
                }
                ("vstack", "padding") | ("hstack", "padding") => {
                    let insets = NSEdgeInsets {
                        top: val_num,
                        left: val_num,
                        bottom: val_num,
                        right: val_num,
                    };
                    let () = msg_send![view, setEdgeInsets: insets];
                }
                (_, "width") => {
                    if val_num > 0.0 {
                        let anchor: *mut Object = msg_send![view, widthAnchor];
                        let constraint: *mut Object =
                            msg_send![anchor, constraintEqualToConstant: val_num];
                        let () = msg_send![constraint, setActive: YES];
                    }
                }
                (_, "height") => {
                    if val_num > 0.0 {
                        let anchor: *mut Object = msg_send![view, heightAnchor];
                        let constraint: *mut Object =
                            msg_send![anchor, constraintEqualToConstant: val_num];
                        let () = msg_send![constraint, setActive: YES];
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn widget_add_child(parent_id: u64, child_id: u64) {
    let pair = {
        let wids = WIDGETS.lock().unwrap();
        wids.as_ref()
            .and_then(|m| m.get(&parent_id).zip(m.get(&child_id)))
            .map(|(p, c)| (p.tag.clone(), p.view.0, c.view.0))
    };
    if let Some((parent_tag, parent_view, child_view)) = pair {
        unsafe {
            if parent_tag == "vstack" || parent_tag == "hstack" {
                let () = msg_send![parent_view, addArrangedSubview: child_view];
            } else if parent_tag == "card" || parent_tag == "box" {
                let () = msg_send![child_view, setAutoresizingMask: 18usize];
                let () = msg_send![parent_view, setContentView: child_view];
            } else if parent_tag == "scroll" || parent_tag == "scrollview" {
                let () = msg_send![parent_view, setDocumentView: child_view];
                let content_view: *mut Object = msg_send![parent_view, contentView];
                let child_width: *mut Object = msg_send![child_view, widthAnchor];
                let clip_width: *mut Object = msg_send![content_view, widthAnchor];
                let constraint: *mut Object =
                    msg_send![child_width, constraintEqualToAnchor: clip_width];
                let () = msg_send![constraint, setActive: YES];
            } else {
                let () = msg_send![parent_view, addSubview: child_view];
            }
        }
    }
}

pub fn widget_remove_child(parent_id: u64, child_id: u64) {
    let pair = {
        let wids = WIDGETS.lock().unwrap();
        wids.as_ref()
            .and_then(|m| m.get(&parent_id).zip(m.get(&child_id)))
            .map(|(p, c)| (p.tag.clone(), p.view.0, c.view.0))
    };
    if let Some((parent_tag, parent_view, child_view)) = pair {
        unsafe {
            if parent_tag == "vstack" || parent_tag == "hstack" {
                let () = msg_send![parent_view, removeArrangedSubview: child_view];
            } else if parent_tag == "scroll" || parent_tag == "scrollview" {
                let () = msg_send![parent_view, setDocumentView: ptr::null_mut::<Object>()];
            }
            let () = msg_send![child_view, removeFromSuperview];
        }
    }
}

pub fn pump_events(wait_ms: f64) -> bool {
    if !ensure_initialized() {
        return false;
    }
    let mut had_events = false;
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let mode = ns_string("kCFRunLoopDefaultMode");

        let until_date: *mut Object = if wait_ms > 0.0 {
            msg_send![class!(NSDate), dateWithTimeIntervalSinceNow: (wait_ms / 1000.0)]
        } else {
            ptr::null_mut::<Object>()
        };

        // First event can block up to wait_ms to give 0.0% CPU usage when idle
        let first_event: *mut Object = msg_send![
            app,
            nextEventMatchingMask: usize::MAX
            untilDate: until_date
            inMode: mode
            dequeue: YES
        ];

        if !first_event.is_null() {
            had_events = true;
            let () = msg_send![app, sendEvent: first_event];

            loop {
                let event: *mut Object = msg_send![
                    app,
                    nextEventMatchingMask: usize::MAX
                    untilDate: ptr::null_mut::<Object>()
                    inMode: mode
                    dequeue: YES
                ];
                if event.is_null() {
                    break;
                }
                let () = msg_send![app, sendEvent: event];
            }
        }
        let () = msg_send![app, updateWindows];
    }

    had_events
}

pub fn poll_event() -> Option<(u64, String, String)> {
    let mut events = PENDING_EVENTS.lock().ok()?;
    if events.is_empty() {
        None
    } else {
        Some(events.remove(0))
    }
}
