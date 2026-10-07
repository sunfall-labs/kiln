//! The AppKit backend of kiln:gui (ADR 0074).
//!
//! Every widget is an `NSView` owned by this module and named by a `u32` handle that this module
//! allocates; the TypeScript side never invents identifiers, so two realms cannot collide. All
//! state is thread-local and initialisation refuses any thread but the main one: AppKit is a
//! main-thread toolkit, and the borrow checker plus `thread_local!` say so without a mutex.
//!
//! Layout is one model for every container: Taffy computes frames from the style object the
//! TypeScript side sends (`layout`), and AppKit only measures leaves. A window's root view
//! re-runs the layout from its `layout` override, which AppKit calls during its display cycle,
//! so live resizing re-flows without a round trip through JavaScript.
//!
//! Re-entrancy: AppKit calls back into this module (actions, delegate methods, `layout`,
//! `updateLayer`). Callbacks only touch [`REGISTRY`], [`PAINTS`] and the event queue, never the
//! main state, except `layout`, which takes the state with `try_borrow_mut` and skips a pass
//! that arrives while an operation holds it.

#![cfg(target_os = "macos")]
#![allow(unexpected_cfgs, clippy::missing_safety_doc)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use objc::declare::ClassDecl;
use objc::rc::autoreleasepool;
use objc::runtime::{BOOL, Class, NO, Object, Sel, YES};
use objc::{class, msg_send, sel, sel_impl};
use serde_json::Value;
use taffy::prelude::{AvailableSpace, Size as TaffySize};

use crate::events::{self, EventKind};
use crate::layout::{Frame, Layouts, parse_style};

#[link(name = "Cocoa", kind = "framework")]
unsafe extern "C" {}

type Id = *mut Object;

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
    CGRect {
        origin: CGPoint { x, y },
        size: CGSize { width, height },
    }
}

const WINDOW_TITLED: usize = 1;
const WINDOW_CLOSABLE: usize = 1 << 1;
const WINDOW_MINIATURIZABLE: usize = 1 << 2;
const WINDOW_RESIZABLE: usize = 1 << 3;
const AUTORESIZE_FILL: usize = 2 | 16; // width | height sizable
const EVENT_MASK_ANY: u64 = u64::MAX;
const SCROLL_DOCUMENT_BIT: u32 = 0x8000_0000;

/// What kind of native view a handle names. Parsed once from the tag the TypeScript side sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Window,
    Container,
    Scroll,
    Text,
    Button,
    TextInput,
    SecureInput,
    TextArea,
    Slider,
    Toggle,
    Checkbox,
    Select,
    Segmented,
    Progress,
    Spinner,
    Image,
    Divider,
    Spacer,
}

impl Kind {
    fn parse(tag: &str) -> Option<Kind> {
        Some(match tag {
            "window" => Kind::Window,
            "container" | "vstack" | "hstack" | "box" | "grid" | "card" => Kind::Container,
            "scroll" => Kind::Scroll,
            "text" => Kind::Text,
            "button" => Kind::Button,
            "textinput" => Kind::TextInput,
            "secureinput" => Kind::SecureInput,
            "textarea" => Kind::TextArea,
            "slider" => Kind::Slider,
            "toggle" => Kind::Toggle,
            "checkbox" => Kind::Checkbox,
            "select" => Kind::Select,
            "segmented" => Kind::Segmented,
            "progress" => Kind::Progress,
            "spinner" => Kind::Spinner,
            "image" => Kind::Image,
            "divider" => Kind::Divider,
            "spacer" => Kind::Spacer,
            _ => return None,
        })
    }

    fn holds_children(self) -> bool {
        matches!(self, Kind::Window | Kind::Container | Kind::Scroll)
    }

    fn is_measured(self) -> bool {
        !matches!(
            self,
            Kind::Window | Kind::Container | Kind::Scroll | Kind::Spacer | Kind::Divider
        )
    }
}

/// The measurement context Taffy hands back for a leaf.
struct Leaf {
    view: Id,
    kind: Kind,
}

#[derive(Clone, Debug)]
struct TextStyle {
    variant: String,
    size: f64,
    weight: String,
    monospace: bool,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            variant: "body".into(),
            size: 0.0,
            weight: String::new(),
            monospace: false,
        }
    }
}

struct Widget {
    kind: Kind,
    /// The view placed in the parent's hierarchy (a window's root container for a window).
    view: Id,
    /// The `NSWindow` of a window; the text view of a text area; the document of a scroll view.
    inner: Id,
    parent: u32,
    text: TextStyle,
}

#[derive(Clone, Default)]
struct Paint {
    background: Option<String>,
    border_color: Option<String>,
    border_width: f64,
    corner_radius: f64,
}

struct App {
    target: Id,
    next_handle: u32,
    widgets: HashMap<u32, Widget>,
    layouts: Layouts<Leaf>,
    /// Windows whose layout must run before the next display.
    dirty: HashSet<u32>,
    activated: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// View or window pointer -> handle, for callbacks that must not borrow [`APP`].
    static REGISTRY: RefCell<HashMap<usize, u32>> = RefCell::new(HashMap::new());
    /// Container view pointer -> paint, read by `updateLayer`.
    static PAINTS: RefCell<HashMap<usize, Paint>> = RefCell::new(HashMap::new());
    /// Root view pointer -> window handle, read by the root's `layout`.
    static ROOTS: RefCell<HashMap<usize, u32>> = RefCell::new(HashMap::new());
    /// Menu item tag -> the id the TypeScript side gave it.
    static MENU_IDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn lookup(object: Id) -> Option<u32> {
    REGISTRY.with(|r| r.borrow().get(&(object as usize)).copied())
}

fn with_app<R>(f: impl FnOnce(&mut App) -> Result<R, String>) -> Result<R, String> {
    let result = APP.with(|cell| {
        let mut guard = cell.try_borrow_mut().map_err(|_| {
            "kiln:gui re-entered the AppKit backend during an operation".to_string()
        })?;
        let app = guard
            .as_mut()
            .ok_or_else(|| "kiln:gui is not initialised on this thread".to_string())?;
        f(app)
    });
    flush_dirty_windows();
    result
}

// ---------------------------------------------------------------------------------------------
// Foundation helpers

unsafe fn ns_string(s: &str) -> Id {
    let c = CString::new(s.replace('\0', "")).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

unsafe fn rust_string(ns: Id) -> String {
    if ns.is_null() {
        return String::new();
    }
    let utf8: *const c_char = unsafe { msg_send![ns, UTF8String] };
    if utf8.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(utf8) }
            .to_string_lossy()
            .into_owned()
    }
}

unsafe fn retain(object: Id) -> Id {
    if !object.is_null() {
        let _: Id = unsafe { msg_send![object, retain] };
    }
    object
}

unsafe fn release(object: Id) {
    if !object.is_null() {
        let () = unsafe { msg_send![object, release] };
    }
}

fn is_main_thread() -> bool {
    let main: BOOL = unsafe { msg_send![class!(NSThread), isMainThread] };
    main == YES
}

/// A semantic name (`"secondary"`, `"accent"`, `"red"`), `"#rgb"`, `"#rrggbb"` or `"#rrggbbaa"`.
unsafe fn color(name: &str) -> Id {
    let c = class!(NSColor);
    let name = name.trim();
    if let Some(hex) = name.strip_prefix('#') {
        let digits: Vec<u8> = hex
            .chars()
            .filter_map(|ch| ch.to_digit(16).map(|d| d as u8))
            .collect();
        let (r, g, b, a) = match digits.len() {
            3 => (digits[0] * 17, digits[1] * 17, digits[2] * 17, 255),
            6 | 8 => {
                let byte = |i: usize| digits[i] * 16 + digits[i + 1];
                (
                    byte(0),
                    byte(2),
                    byte(4),
                    if digits.len() == 8 { byte(6) } else { 255 },
                )
            }
            _ => (0, 0, 0, 255),
        };
        let f = |v: u8| v as f64 / 255.0;
        return unsafe { msg_send![c, colorWithSRGBRed: f(r) green: f(g) blue: f(b) alpha: f(a)] };
    }
    unsafe {
        match name {
            "label" | "primary" | "text" | "default" => msg_send![c, labelColor],
            "secondary" => msg_send![c, secondaryLabelColor],
            "tertiary" => msg_send![c, tertiaryLabelColor],
            "quaternary" => msg_send![c, quaternaryLabelColor],
            "accent" | "tint" => msg_send![c, controlAccentColor],
            "separator" => msg_send![c, separatorColor],
            "background" | "window" => msg_send![c, windowBackgroundColor],
            "control" | "content" => msg_send![c, controlBackgroundColor],
            "fill" | "card" => msg_send![c, quaternaryLabelColor],
            "selection" => msg_send![c, selectedContentBackgroundColor],
            "link" => msg_send![c, linkColor],
            "red" | "danger" | "error" | "destructive" => msg_send![c, systemRedColor],
            "orange" | "warning" => msg_send![c, systemOrangeColor],
            "yellow" => msg_send![c, systemYellowColor],
            "green" | "success" => msg_send![c, systemGreenColor],
            "mint" => msg_send![c, systemMintColor],
            "teal" => msg_send![c, systemTealColor],
            "cyan" => msg_send![c, systemCyanColor],
            "blue" | "info" => msg_send![c, systemBlueColor],
            "indigo" => msg_send![c, systemIndigoColor],
            "purple" => msg_send![c, systemPurpleColor],
            "pink" => msg_send![c, systemPinkColor],
            "brown" => msg_send![c, systemBrownColor],
            "gray" | "grey" => msg_send![c, systemGrayColor],
            "white" => msg_send![c, whiteColor],
            "black" => msg_send![c, blackColor],
            "clear" | "transparent" => msg_send![c, clearColor],
            _ => msg_send![c, labelColor],
        }
    }
}

/// SF Symbol names, with a few short aliases.
fn symbol_name(name: &str) -> &str {
    match name {
        "settings" | "gear" => "gearshape",
        "search" => "magnifyingglass",
        "play" => "play.fill",
        "pause" => "pause.fill",
        "stop" => "stop.fill",
        "refresh" | "reload" => "arrow.clockwise",
        "terminal" => "terminal.fill",
        "memory" => "memorychip",
        "check" | "checkmark" | "success" => "checkmark.circle.fill",
        "error" | "danger" | "warning" => "exclamationmark.triangle.fill",
        "info" => "info.circle.fill",
        "folder" => "folder.fill",
        "file" => "doc.text.fill",
        "add" | "plus" => "plus",
        "remove" | "minus" => "minus",
        "delete" | "trash" => "trash",
        "close" | "xmark" => "xmark",
        "edit" => "pencil",
        "star" => "star.fill",
        "heart" => "heart.fill",
        "user" | "person" => "person.crop.circle",
        other => other,
    }
}

unsafe fn image_for(name: &str) -> Id {
    let is_path = name.starts_with('/')
        || name.starts_with("./")
        || name.starts_with("../")
        || name.starts_with('~');
    if is_path {
        let alloc: Id = unsafe { msg_send![class!(NSImage), alloc] };
        let image: Id = unsafe { msg_send![alloc, initWithContentsOfFile: ns_string(name)] };
        return image; // +1, caller releases after handing it to a view
    }
    let image: Id = unsafe {
        msg_send![class!(NSImage), imageWithSystemSymbolName: ns_string(symbol_name(name))
                                    accessibilityDescription: ptr::null_mut::<Object>()]
    };
    unsafe { retain(image) }
}

fn font_weight(name: &str) -> f64 {
    match name {
        "ultralight" => -0.8,
        "thin" => -0.6,
        "light" => -0.4,
        "medium" => 0.23,
        "semibold" => 0.3,
        "bold" => 0.4,
        "heavy" => 0.56,
        "black" => 0.62,
        _ => 0.0,
    }
}

unsafe fn font_for(style: &TextStyle) -> Id {
    // macOS type ramp; titles a step heavier than the HIG so hierarchy reads at a glance.
    let (size, weight) = match style.variant.as_str() {
        "largeTitle" => (26.0, "bold"),
        "title" => (22.0, "bold"),
        "title2" => (17.0, "semibold"),
        "title3" => (15.0, "semibold"),
        "headline" | "bold" => (13.0, "bold"),
        "subheadline" => (11.0, "regular"),
        "callout" => (12.0, "regular"),
        "footnote" | "caption" | "caption2" => (10.0, "regular"),
        "badge" => (11.0, "bold"),
        _ => (13.0, "regular"),
    };
    let size = if style.size > 0.0 { style.size } else { size };
    let weight = font_weight(if style.weight.is_empty() {
        weight
    } else {
        &style.weight
    });
    unsafe {
        if style.monospace {
            msg_send![class!(NSFont), monospacedSystemFontOfSize: size weight: weight]
        } else {
            msg_send![class!(NSFont), systemFontOfSize: size weight: weight]
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Classes: the flipped container, the root container, the clip view and the action target.

extern "C" fn yes(_: &Object, _: Sel) -> BOOL {
    YES
}

extern "C" fn update_layer(this: &Object, _: Sel) {
    let key = this as *const Object as usize;
    let Some(paint) = PAINTS.with(|p| p.borrow().get(&key).cloned()) else {
        return;
    };
    unsafe {
        let layer: Id = msg_send![this, layer];
        if layer.is_null() {
            return;
        }
        let cg = |name: &Option<String>| -> *const std::ffi::c_void {
            match name {
                Some(name) => {
                    let ns = color(name);
                    msg_send![ns, CGColor]
                }
                None => ptr::null(),
            }
        };
        let () = msg_send![layer, setBackgroundColor: cg(&paint.background)];
        let () = msg_send![layer, setBorderColor: cg(&paint.border_color)];
        let () = msg_send![layer, setBorderWidth: paint.border_width];
        let () = msg_send![layer, setCornerRadius: paint.corner_radius];
        let () = msg_send![layer, setMasksToBounds: (paint.corner_radius > 0.0) as BOOL];
    }
}

extern "C" fn root_layout(this: &Object, _: Sel) {
    unsafe {
        let () = msg_send![super(this, class!(NSView)), layout];
    }
    let key = this as *const Object as usize;
    if let Some(window) = ROOTS.with(|r| r.borrow().get(&key).copied()) {
        APP.with(|cell| {
            if let Ok(mut guard) = cell.try_borrow_mut()
                && let Some(app) = guard.as_mut()
            {
                layout_window(app, window);
            }
        });
    }
}

/// VoiceOver (or any assistive technology) pressing a pressable container: the same event a
/// click delivers.
extern "C" fn accessibility_press(this: &Object, _: Sel) -> BOOL {
    match lookup(this as *const Object as *mut Object) {
        Some(h) => {
            push(h, EventKind::Press, 0.0, 0.0, String::new());
            YES
        }
        None => NO,
    }
}

fn view_class(name: &str, root: bool) -> &'static Class {
    if let Some(class) = Class::get(name) {
        return class;
    }
    let mut decl = ClassDecl::new(name, class!(NSView)).expect("register kiln view class");
    unsafe {
        decl.add_method(sel!(isFlipped), yes as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_method(
            sel!(wantsUpdateLayer),
            yes as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(updateLayer),
            update_layer as extern "C" fn(&Object, Sel),
        );
        decl.add_method(
            sel!(accessibilityPerformPress),
            accessibility_press as extern "C" fn(&Object, Sel) -> BOOL,
        );
        if root {
            decl.add_method(sel!(layout), root_layout as extern "C" fn(&Object, Sel));
        }
    }
    decl.register()
}

fn clip_class() -> &'static Class {
    if let Some(class) = Class::get("KilnClipView") {
        return class;
    }
    let mut decl = ClassDecl::new("KilnClipView", class!(NSClipView)).expect("register clip class");
    unsafe {
        decl.add_method(sel!(isFlipped), yes as extern "C" fn(&Object, Sel) -> BOOL);
    }
    decl.register()
}

fn push(handle: u32, kind: EventKind, a: f64, b: f64, text: String) {
    events::push(events::Event {
        handle,
        kind,
        a,
        b,
        text,
    });
}

extern "C" fn on_press(_: &Object, _: Sel, sender: Id) {
    if let Some(h) = lookup(sender) {
        push(h, EventKind::Press, 0.0, 0.0, String::new());
    }
}

extern "C" fn on_tap(_: &Object, _: Sel, recognizer: Id) {
    let view: Id = unsafe { msg_send![recognizer, view] };
    if let Some(h) = lookup(view) {
        push(h, EventKind::Press, 0.0, 0.0, String::new());
    }
}

extern "C" fn on_value(_: &Object, _: Sel, sender: Id) {
    let Some(h) = lookup(sender) else { return };
    unsafe {
        let is = |name: &str| -> bool {
            let other = Class::get(name)
                .map(|c| c as *const Class)
                .unwrap_or(ptr::null());
            let yes: BOOL = msg_send![sender, isKindOfClass: other];
            yes == YES
        };
        if is("NSSlider") {
            let v: f64 = msg_send![sender, doubleValue];
            push(h, EventKind::Change, v, 0.0, String::new());
        } else if is("NSPopUpButton") {
            let i: isize = msg_send![sender, indexOfSelectedItem];
            let title: Id = msg_send![sender, titleOfSelectedItem];
            push(h, EventKind::Change, i as f64, 0.0, rust_string(title));
        } else if is("NSSegmentedControl") {
            let i: isize = msg_send![sender, selectedSegment];
            let label: Id = if i >= 0 {
                msg_send![sender, labelForSegment: i]
            } else {
                ptr::null_mut()
            };
            push(h, EventKind::Change, i as f64, 0.0, rust_string(label));
        } else if is("NSTextField") {
            let s: Id = msg_send![sender, stringValue];
            push(h, EventKind::Submit, 0.0, 0.0, rust_string(s));
        } else {
            // NSSwitch and checkbox buttons.
            let state: isize = msg_send![sender, state];
            push(
                h,
                EventKind::Change,
                if state == 1 { 1.0 } else { 0.0 },
                0.0,
                String::new(),
            );
        }
    }
}

extern "C" fn on_text_change(_: &Object, _: Sel, notification: Id) {
    unsafe {
        let field: Id = msg_send![notification, object];
        if let Some(h) = lookup(field) {
            let s: Id = msg_send![field, stringValue];
            push(h, EventKind::Input, 0.0, 0.0, rust_string(s));
        }
    }
}

extern "C" fn on_text_view_change(_: &Object, _: Sel, notification: Id) {
    unsafe {
        let view: Id = msg_send![notification, object];
        if let Some(h) = lookup(view) {
            let s: Id = msg_send![view, string];
            push(h, EventKind::Input, 0.0, 0.0, rust_string(s));
        }
    }
}

extern "C" fn on_window_close(_: &Object, _: Sel, notification: Id) {
    unsafe {
        let window: Id = msg_send![notification, object];
        if let Some(h) = lookup(window) {
            push(h, EventKind::Close, 0.0, 0.0, String::new());
        }
    }
}

extern "C" fn on_window_resize(_: &Object, _: Sel, notification: Id) {
    unsafe {
        let window: Id = msg_send![notification, object];
        if let Some(h) = lookup(window) {
            let content: Id = msg_send![window, contentView];
            let bounds: CGRect = msg_send![content, bounds];
            push(
                h,
                EventKind::Resize,
                bounds.size.width,
                bounds.size.height,
                String::new(),
            );
        }
    }
}

extern "C" fn on_menu(_: &Object, _: Sel, sender: Id) {
    let tag: isize = unsafe { msg_send![sender, tag] };
    let id = MENU_IDS.with(|ids| ids.borrow().get(tag as usize).cloned());
    if let Some(id) = id {
        push(0, EventKind::Menu, 0.0, 0.0, id);
    }
}

extern "C" fn on_quit(_: &Object, _: Sel, _sender: Id) {
    push(0, EventKind::Quit, 0.0, 0.0, String::new());
}

extern "C" fn should_terminate(_: &Object, _: Sel, _app: Id) -> usize {
    // NSTerminateCancel: the program decides when to end, through the quit event.
    push(0, EventKind::Quit, 0.0, 0.0, String::new());
    0
}

fn target_class() -> &'static Class {
    if let Some(class) = Class::get("KilnTarget") {
        return class;
    }
    let mut decl = ClassDecl::new("KilnTarget", class!(NSObject)).expect("register target class");
    unsafe {
        type Action = extern "C" fn(&Object, Sel, Id);
        decl.add_method(sel!(kilnPress:), on_press as Action);
        decl.add_method(sel!(kilnValue:), on_value as Action);
        decl.add_method(sel!(kilnTap:), on_tap as Action);
        decl.add_method(sel!(kilnMenu:), on_menu as Action);
        decl.add_method(sel!(kilnQuit:), on_quit as Action);
        decl.add_method(sel!(controlTextDidChange:), on_text_change as Action);
        decl.add_method(sel!(textDidChange:), on_text_view_change as Action);
        decl.add_method(sel!(windowWillClose:), on_window_close as Action);
        decl.add_method(sel!(windowDidResize:), on_window_resize as Action);
        decl.add_method(
            sel!(applicationShouldTerminate:),
            should_terminate as extern "C" fn(&Object, Sel, Id) -> usize,
        );
    }
    decl.register()
}

// ---------------------------------------------------------------------------------------------
// Lifecycle

pub fn init() -> Result<(), String> {
    if !is_main_thread() {
        return Err("AppKit runs only on the main thread, and this realm is not on it".into());
    }
    let initialised = APP.with(|cell| cell.borrow().is_some());
    if initialised {
        return Ok(());
    }
    autoreleasepool(|| unsafe {
        let app: Id = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return Err("NSApplication is unavailable".to_string());
        }
        let () = msg_send![app, setActivationPolicy: 0isize]; // regular: dock icon and menu bar
        let target: Id = msg_send![target_class(), new];
        let () = msg_send![app, setDelegate: target];
        install_menu(app, target, &[]);
        let () = msg_send![app, finishLaunching];
        APP.with(|cell| {
            *cell.borrow_mut() = Some(App {
                target,
                next_handle: 1,
                widgets: HashMap::new(),
                layouts: Layouts::new(),
                dirty: HashSet::new(),
                activated: false,
            })
        });
        Ok(())
    })
}

pub fn create(tag: &str) -> Result<u32, String> {
    let kind = Kind::parse(tag).ok_or_else(|| format!("AppKit has no widget for <{tag}>"))?;
    with_app(|app| autoreleasepool(|| unsafe { create_widget(app, kind) }))
}

unsafe fn create_widget(app: &mut App, kind: Kind) -> Result<u32, String> {
    let handle = app.next_handle;
    app.next_handle += 1;
    let target = app.target;
    let frame = rect(0.0, 0.0, 100.0, 24.0);
    let mut inner: Id = ptr::null_mut();
    let view: Id = unsafe {
        match kind {
            Kind::Window => {
                let style =
                    WINDOW_TITLED | WINDOW_CLOSABLE | WINDOW_MINIATURIZABLE | WINDOW_RESIZABLE;
                let alloc: Id = msg_send![class!(NSWindow), alloc];
                let window: Id = msg_send![alloc, initWithContentRect: rect(0.0, 0.0, 800.0, 600.0)
                                                          styleMask: style
                                                            backing: 2usize
                                                              defer: NO];
                let () = msg_send![window, setReleasedWhenClosed: NO];
                let () = msg_send![window, setDelegate: target];
                let content: Id = msg_send![window, contentView];
                let bounds: CGRect = msg_send![content, bounds];
                let alloc: Id = msg_send![class!(NSVisualEffectView), alloc];
                let effect: Id = msg_send![alloc, initWithFrame: bounds];
                let () = msg_send![effect, setMaterial: 12isize]; // window background
                let () = msg_send![effect, setBlendingMode: 0isize];
                let () = msg_send![effect, setState: 0isize]; // follow window active state
                let () = msg_send![effect, setAutoresizingMask: AUTORESIZE_FILL];
                let () = msg_send![window, setContentView: effect];
                let () = msg_send![effect, release];
                let root_alloc: Id = msg_send![view_class("KilnRootView", true), alloc];
                let root: Id = msg_send![root_alloc, initWithFrame: bounds];
                let () = msg_send![root, setAutoresizingMask: AUTORESIZE_FILL];
                let () = msg_send![root, setWantsLayer: YES];
                let () = msg_send![effect, addSubview: root];
                let () = msg_send![window, center];
                // Tab and Shift-Tab move through the controls in layout order.
                let () = msg_send![window, setAutorecalculatesKeyViewLoop: YES];
                inner = window;
                ROOTS.with(|r| r.borrow_mut().insert(root as usize, handle));
                REGISTRY.with(|r| r.borrow_mut().insert(window as usize, handle));
                root
            }
            Kind::Container | Kind::Spacer => {
                let alloc: Id = msg_send![view_class("KilnView", false), alloc];
                let view: Id = msg_send![alloc, initWithFrame: frame];
                let () = msg_send![view, setWantsLayer: YES];
                view
            }
            Kind::Scroll => {
                let alloc: Id = msg_send![class!(NSScrollView), alloc];
                let scroll: Id = msg_send![alloc, initWithFrame: rect(0.0, 0.0, 200.0, 200.0)];
                let clip_alloc: Id = msg_send![clip_class(), alloc];
                let clip: Id = msg_send![clip_alloc, initWithFrame: rect(0.0, 0.0, 200.0, 200.0)];
                let () = msg_send![clip, setDrawsBackground: NO];
                let () = msg_send![scroll, setContentView: clip];
                let () = msg_send![clip, release];
                let () = msg_send![scroll, setHasVerticalScroller: YES];
                let () = msg_send![scroll, setAutohidesScrollers: YES];
                let () = msg_send![scroll, setDrawsBackground: NO];
                let doc_alloc: Id = msg_send![view_class("KilnView", false), alloc];
                let doc: Id = msg_send![doc_alloc, initWithFrame: rect(0.0, 0.0, 200.0, 10.0)];
                let () = msg_send![scroll, setDocumentView: doc];
                inner = doc; // retained by us as well as by the scroll view
                scroll
            }
            Kind::Text => {
                let label: Id = msg_send![class!(NSTextField), labelWithString: ns_string("")];
                let label = retain(label);
                let () = msg_send![label, setLineBreakMode: 0isize]; // word wrapping
                let () = msg_send![label, setMaximumNumberOfLines: 0isize];
                let cell: Id = msg_send![label, cell];
                let () = msg_send![cell, setWraps: YES];
                let () = msg_send![label, setFont: font_for(&TextStyle::default())];
                label
            }
            Kind::Button => {
                let button: Id = msg_send![class!(NSButton), buttonWithTitle: ns_string("")
                                                                       target: target
                                                                       action: sel!(kilnPress:)];
                let button = retain(button);
                let () = msg_send![button, setBezelStyle: 1isize]; // rounded push button
                button
            }
            Kind::TextInput | Kind::SecureInput => {
                let class = if kind == Kind::SecureInput {
                    class!(NSSecureTextField)
                } else {
                    class!(NSTextField)
                };
                let alloc: Id = msg_send![class, alloc];
                let field: Id = msg_send![alloc, initWithFrame: frame];
                let () = msg_send![field, setBezeled: YES];
                let () = msg_send![field, setBezelStyle: 1isize]; // rounded
                let () = msg_send![field, setEditable: YES];
                let () = msg_send![field, setSelectable: YES];
                let () = msg_send![field, setDelegate: target];
                let () = msg_send![field, setTarget: target];
                let () = msg_send![field, setAction: sel!(kilnValue:)];
                let cell: Id = msg_send![field, cell];
                let () = msg_send![cell, setScrollable: YES];
                let () = msg_send![field, setFont: font_for(&TextStyle::default())];
                field
            }
            Kind::TextArea => {
                let scroll: Id = msg_send![class!(NSTextView), scrollableTextView];
                let scroll = retain(scroll);
                let text_view: Id = msg_send![scroll, documentView];
                let () = msg_send![text_view, setDelegate: target];
                let () = msg_send![text_view, setRichText: NO];
                let () = msg_send![text_view, setAllowsUndo: YES];
                let () = msg_send![text_view, setFont: font_for(&TextStyle::default())];
                let () =
                    msg_send![text_view, setTextContainerInset: CGSize { width: 4.0, height: 6.0 }];
                let () = msg_send![scroll, setBorderType: 2usize]; // bezel
                REGISTRY.with(|r| r.borrow_mut().insert(text_view as usize, handle));
                inner = retain(text_view);
                scroll
            }
            Kind::Slider => {
                let slider: Id = msg_send![class!(NSSlider), sliderWithValue: 0.0f64
                                                                    minValue: 0.0f64
                                                                    maxValue: 100.0f64
                                                                      target: target
                                                                      action: sel!(kilnValue:)];
                let slider = retain(slider);
                let () = msg_send![slider, setContinuous: YES];
                slider
            }
            Kind::Toggle => {
                let alloc: Id = msg_send![class!(NSSwitch), alloc];
                let switch: Id = msg_send![alloc, initWithFrame: frame];
                let () = msg_send![switch, setTarget: target];
                let () = msg_send![switch, setAction: sel!(kilnValue:)];
                switch
            }
            Kind::Checkbox => {
                let button: Id = msg_send![class!(NSButton), checkboxWithTitle: ns_string("")
                                                                         target: target
                                                                         action: sel!(kilnValue:)];
                retain(button)
            }
            Kind::Select => {
                let alloc: Id = msg_send![class!(NSPopUpButton), alloc];
                let popup: Id = msg_send![alloc, initWithFrame: frame pullsDown: NO];
                let () = msg_send![popup, setTarget: target];
                let () = msg_send![popup, setAction: sel!(kilnValue:)];
                popup
            }
            Kind::Segmented => {
                let alloc: Id = msg_send![class!(NSSegmentedControl), alloc];
                let seg: Id = msg_send![alloc, initWithFrame: frame];
                let () = msg_send![seg, setSegmentStyle: 1isize]; // rounded
                let () = msg_send![seg, setTrackingMode: 0isize]; // select one
                let () = msg_send![seg, setTarget: target];
                let () = msg_send![seg, setAction: sel!(kilnValue:)];
                seg
            }
            Kind::Progress | Kind::Spinner => {
                let alloc: Id = msg_send![class!(NSProgressIndicator), alloc];
                let bar: Id = msg_send![alloc, initWithFrame: rect(0.0, 0.0, 120.0, 20.0)];
                let spinner = kind == Kind::Spinner;
                let () = msg_send![bar, setStyle: if spinner { 1usize } else { 0usize }];
                let () = msg_send![bar, setIndeterminate: if spinner { YES } else { NO }];
                let () = msg_send![bar, setMinValue: 0.0f64];
                let () = msg_send![bar, setMaxValue: 100.0f64];
                if spinner {
                    let () = msg_send![bar, setControlSize: 1usize]; // small
                    let () = msg_send![bar, sizeToFit];
                    let () = msg_send![bar, startAnimation: ptr::null_mut::<Object>()];
                }
                bar
            }
            Kind::Image => {
                let alloc: Id = msg_send![class!(NSImageView), alloc];
                let image: Id = msg_send![alloc, initWithFrame: rect(0.0, 0.0, 16.0, 16.0)];
                let () = msg_send![image, setImageScaling: 2usize]; // proportionally up or down
                // Decorative until labelled: VoiceOver should not announce "image" for an icon.
                let () = msg_send![image, setAccessibilityElement: NO];
                image
            }
            Kind::Divider => {
                let alloc: Id = msg_send![class!(NSBox), alloc];
                let line: Id = msg_send![alloc, initWithFrame: rect(0.0, 0.0, 100.0, 1.0)];
                let () = msg_send![line, setBoxType: 2usize]; // separator
                line
            }
        }
    };
    if kind != Kind::Window {
        REGISTRY.with(|r| r.borrow_mut().insert(view as usize, handle));
    }
    let leaf = kind.is_measured().then_some(Leaf { view, kind });
    app.layouts.create(handle, Default::default(), leaf);
    if kind == Kind::Scroll {
        app.layouts
            .create(handle | SCROLL_DOCUMENT_BIT, Default::default(), None);
    }
    app.widgets.insert(
        handle,
        Widget {
            kind,
            view,
            inner,
            parent: 0,
            text: TextStyle::default(),
        },
    );
    Ok(handle)
}

fn window_of(app: &App, mut handle: u32) -> Option<u32> {
    for _ in 0..4096 {
        let widget = app.widgets.get(&handle)?;
        if widget.kind == Kind::Window {
            return Some(handle);
        }
        handle = widget.parent;
    }
    None
}

fn invalidate(app: &mut App, handle: u32) {
    if let Some(window) = window_of(app, handle) {
        app.dirty.insert(window);
    }
}

/// Asks AppKit to lay out every window an operation touched, after the operation released the
/// state, so that the root's `layout` override can take it.
fn flush_dirty_windows() {
    let roots: Vec<Id> = APP.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return Vec::new();
        };
        let Some(app) = guard.as_mut() else {
            return Vec::new();
        };
        let dirty: Vec<u32> = app.dirty.drain().collect();
        dirty
            .into_iter()
            .filter_map(|w| app.widgets.get(&w).map(|w| w.view))
            .collect()
    });
    for root in roots {
        unsafe {
            let () = msg_send![root, setNeedsLayout: YES];
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Layout

fn measure(
    leaf: &mut Leaf,
    known: TaffySize<Option<f32>>,
    available: TaffySize<AvailableSpace>,
) -> TaffySize<f32> {
    unsafe {
        let view = leaf.view;
        let size = if leaf.kind == Kind::Text {
            let width = match (known.width, available.width) {
                (Some(w), _) => w as f64,
                (None, AvailableSpace::Definite(w)) => w as f64,
                (None, AvailableSpace::MinContent) => 1.0,
                (None, AvailableSpace::MaxContent) => 1.0e7,
            };
            let cell: Id = msg_send![view, cell];
            let fitted: CGSize = msg_send![cell, cellSizeForBounds: rect(0.0, 0.0, width, 1.0e7)];
            fitted
        } else {
            let intrinsic: CGSize = msg_send![view, intrinsicContentSize];
            let fitting: CGSize = msg_send![view, fittingSize];
            CGSize {
                width: if intrinsic.width > 0.0 {
                    intrinsic.width
                } else {
                    fitting.width.max(0.0)
                },
                height: if intrinsic.height > 0.0 {
                    intrinsic.height
                } else {
                    fitting.height.max(0.0)
                },
            }
        };
        TaffySize {
            width: known.width.unwrap_or(size.width.ceil() as f32),
            height: known.height.unwrap_or(size.height.ceil() as f32),
        }
    }
}

fn layout_window(app: &mut App, window: u32) {
    let Some(root) = app.widgets.get(&window).map(|w| w.view) else {
        return;
    };
    let bounds: CGRect = unsafe { msg_send![root, bounds] };
    app.layouts.compute(
        window,
        Some(bounds.size.width),
        Some(bounds.size.height),
        measure,
    );
    place_children(app, window);
}

fn set_frame(view: Id, frame: Frame) {
    unsafe {
        let () = msg_send![view, setFrame: rect(frame.x, frame.y, frame.width, frame.height)];
    }
}

fn place_children(app: &mut App, parent: u32) {
    for child in app.layouts.children(parent) {
        let (Some(frame), Some(widget)) = (app.layouts.frame(child), app.widgets.get(&child))
        else {
            continue;
        };
        let (kind, view) = (widget.kind, widget.view);
        set_frame(view, frame);
        match kind {
            Kind::Container => place_children(app, child),
            Kind::Scroll => layout_scroll(app, child),
            _ => {}
        }
    }
}

/// A scroll view's document is its own layout root: as wide as the visible area, as tall as its
/// content.
fn layout_scroll(app: &mut App, handle: u32) {
    let Some(widget) = app.widgets.get(&handle) else {
        return;
    };
    let (scroll, doc) = (widget.view, widget.inner);
    let document = handle | SCROLL_DOCUMENT_BIT;
    unsafe {
        let () = msg_send![scroll, tile];
        let clip: Id = msg_send![scroll, contentView];
        let visible: CGRect = msg_send![clip, bounds];
        app.layouts
            .compute(document, Some(visible.size.width), None, measure);
        let (_, height) = app.layouts.content_size(document).unwrap_or((0.0, 0.0));
        let height = height.max(visible.size.height);
        let () = msg_send![doc, setFrame: rect(0.0, 0.0, visible.size.width, height)];
    }
    place_children(app, document);
}

// ---------------------------------------------------------------------------------------------
// Properties

pub fn set_text(handle: u32, name: &str, value: &str) -> Result<(), String> {
    with_app(|app| {
        autoreleasepool(|| unsafe { apply(app, handle, name, Value::String(value.to_string())) })
    })
}

pub fn set_number(handle: u32, name: &str, value: f64) -> Result<(), String> {
    let value = serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null);
    with_app(|app| autoreleasepool(|| unsafe { apply(app, handle, name, value) }))
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        Value::String(s) => !s.is_empty() && s != "false" && s != "0",
        _ => false,
    }
}

fn num(value: &Value) -> f64 {
    match value {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        Value::Bool(b) => *b as i32 as f64,
        _ => 0.0,
    }
}

fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn string_list(value: &Value) -> Vec<String> {
    let parsed;
    let value = match value {
        Value::String(s) if s.trim_start().starts_with('[') => {
            parsed = serde_json::from_str(s).unwrap_or(Value::Null);
            &parsed
        }
        other => other,
    };
    match value {
        Value::Array(items) => items.iter().map(text).collect(),
        Value::String(s) => s.split(',').map(|s| s.trim().to_string()).collect(),
        _ => Vec::new(),
    }
}

unsafe fn apply(app: &mut App, handle: u32, name: &str, value: Value) -> Result<(), String> {
    let widget = app
        .widgets
        .get_mut(&handle)
        .ok_or_else(|| format!("no widget with handle {handle}"))?;
    let (kind, view, inner) = (widget.kind, widget.view, widget.inner);
    let mut relayout = false;
    unsafe {
        match (kind, name) {
            (_, "layout") => {
                let style = match &value {
                    Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
                    other => other.clone(),
                };
                app.layouts.set_style(handle, parse_style(&style));
                if kind == Kind::Scroll {
                    let doc = serde_json::json!({ "flexDirection": "column", "alignItems": "stretch",
                        "padding": style.get("padding").cloned().unwrap_or(Value::Null),
                        "gap": style.get("gap").cloned().unwrap_or(Value::Null) });
                    app.layouts
                        .set_style(handle | SCROLL_DOCUMENT_BIT, parse_style(&doc));
                }
                relayout = true;
            }
            (_, "hidden") => {
                let () = msg_send![view, setHidden: truthy(&value) as BOOL];
                relayout = true;
            }
            (_, "accessibilityLabel" | "accessibilityHint") => {
                let target = if kind == Kind::TextArea { inner } else { view };
                let text = ns_string(&text(&value));
                if name == "accessibilityLabel" {
                    let () = msg_send![target, setAccessibilityLabel: text];
                    if kind == Kind::Image {
                        let () = msg_send![view, setAccessibilityElement: YES];
                    }
                } else {
                    let () = msg_send![target, setAccessibilityHelp: text];
                }
            }
            (_, "tooltip") => {
                let () = msg_send![view, setToolTip: ns_string(&text(&value))];
            }
            (_, "opacity") => {
                let () = msg_send![view, setAlphaValue: num(&value)];
            }
            (
                Kind::Window | Kind::Container | Kind::Spacer,
                "background" | "borderColor" | "borderWidth" | "cornerRadius",
            ) => {
                PAINTS.with(|paints| {
                    let mut paints = paints.borrow_mut();
                    let paint = paints.entry(view as usize).or_default();
                    match name {
                        "background" => {
                            paint.background = Some(text(&value)).filter(|s| !s.is_empty())
                        }
                        "borderColor" => {
                            paint.border_color = Some(text(&value)).filter(|s| !s.is_empty())
                        }
                        "borderWidth" => paint.border_width = num(&value),
                        _ => paint.corner_radius = num(&value),
                    }
                });
                let () = msg_send![view, setNeedsDisplay: YES];
            }
            (Kind::Container | Kind::Window, "pressable") => {
                let recognizers: Id = msg_send![view, gestureRecognizers];
                let count: usize = msg_send![recognizers, count];
                if truthy(&value) && count == 0 {
                    let alloc: Id = msg_send![class!(NSClickGestureRecognizer), alloc];
                    let click: Id =
                        msg_send![alloc, initWithTarget: app.target action: sel!(kilnTap:)];
                    let () = msg_send![view, addGestureRecognizer: click];
                    let () = msg_send![click, release];
                    let () = msg_send![view, setAccessibilityElement: YES];
                    let () = msg_send![view, setAccessibilityRole: ns_string("AXButton")];
                } else if !truthy(&value) {
                    let () = msg_send![view, setAccessibilityElement: NO];
                    for i in (0..count).rev() {
                        let r: Id = msg_send![recognizers, objectAtIndex: i];
                        let () = msg_send![view, removeGestureRecognizer: r];
                    }
                }
            }
            (_, "disabled") => {
                let target = if kind == Kind::TextArea { inner } else { view };
                let responds: BOOL = msg_send![target, respondsToSelector: sel!(setEnabled:)];
                if responds == YES {
                    let () = msg_send![target, setEnabled: (!truthy(&value)) as BOOL];
                } else if kind == Kind::TextArea {
                    let () = msg_send![inner, setEditable: (!truthy(&value)) as BOOL];
                }
            }

            // Window
            (Kind::Window, "title") => {
                let () = msg_send![inner, setTitle: ns_string(&text(&value))];
            }
            (Kind::Window, "width" | "height") => {
                let content: Id = msg_send![inner, contentView];
                let bounds: CGRect = msg_send![content, bounds];
                let mut size = bounds.size;
                if name == "width" {
                    size.width = num(&value)
                } else {
                    size.height = num(&value)
                }
                let () = msg_send![inner, setContentSize: size];
                relayout = true;
            }
            (Kind::Window, "minWidth" | "minHeight") => {
                let mut size: CGSize = msg_send![inner, contentMinSize];
                if name == "minWidth" {
                    size.width = num(&value)
                } else {
                    size.height = num(&value)
                }
                let () = msg_send![inner, setContentMinSize: size];
            }
            (Kind::Window, "resizable") => {
                let mut mask: usize = msg_send![inner, styleMask];
                if truthy(&value) {
                    mask |= WINDOW_RESIZABLE
                } else {
                    mask &= !WINDOW_RESIZABLE
                }
                let () = msg_send![inner, setStyleMask: mask];
            }
            (Kind::Window, "material") => {
                let effect: Id = msg_send![inner, contentView];
                let material: isize = match text(&value).as_str() {
                    "sidebar" => 7,
                    "underWindow" | "translucent" => 21,
                    "hud" => 13,
                    "content" => 18,
                    "titlebar" => 3,
                    _ => 12,
                };
                let () = msg_send![effect, setMaterial: material];
            }

            // Text
            (Kind::Text, "text") => {
                let () = msg_send![view, setStringValue: ns_string(&text(&value))];
                app.layouts.mark_dirty(handle);
                relayout = true;
            }
            (
                Kind::Text | Kind::TextInput | Kind::SecureInput | Kind::TextArea,
                "variant" | "size" | "weight" | "monospace",
            ) => {
                let style = &mut app.widgets.get_mut(&handle).expect("widget").text;
                match name {
                    "variant" => style.variant = text(&value),
                    "size" => style.size = num(&value),
                    "weight" => style.weight = text(&value),
                    _ => style.monospace = truthy(&value),
                }
                let font = font_for(style);
                let target = if kind == Kind::TextArea { inner } else { view };
                let () = msg_send![target, setFont: font];
                app.layouts.mark_dirty(handle);
                relayout = true;
            }
            (Kind::Text | Kind::TextInput | Kind::SecureInput, "color") => {
                let () = msg_send![view, setTextColor: color(&text(&value))];
            }
            (Kind::TextArea, "color") => {
                let () = msg_send![inner, setTextColor: color(&text(&value))];
            }
            (Kind::Text | Kind::TextInput | Kind::SecureInput, "align") => {
                // NSTextAlignment: left 0, right 1, center 2, justified 3, natural 4.
                let alignment: isize = match text(&value).as_str() {
                    "center" => 2,
                    "end" | "right" | "trailing" => 1,
                    "justify" | "justified" => 3,
                    "left" => 0,
                    _ => 4,
                };
                let () = msg_send![view, setAlignment: alignment];
            }
            (Kind::Text, "lines") => {
                let lines = num(&value).max(0.0) as isize;
                let () = msg_send![view, setMaximumNumberOfLines: lines];
                let () =
                    msg_send![view, setLineBreakMode: if lines == 1 { 4isize } else { 0isize }];
                let cell: Id = msg_send![view, cell];
                let () = msg_send![cell, setWraps: (lines != 1) as BOOL];
                let () = msg_send![cell, setTruncatesLastVisibleLine: YES];
                app.layouts.mark_dirty(handle);
                relayout = true;
            }
            (Kind::Text, "selectable") => {
                let () = msg_send![view, setSelectable: truthy(&value) as BOOL];
            }

            // Button
            (Kind::Button, "label") => {
                let () = msg_send![view, setTitle: ns_string(&text(&value))];
                app.layouts.mark_dirty(handle);
                relayout = true;
            }
            (Kind::Button, "variant") => {
                let variant = text(&value);
                let borderless = matches!(variant.as_str(), "plain" | "link");
                let () = msg_send![view, setBordered: (!borderless) as BOOL];
                let key = if variant == "primary" { "\r" } else { "" };
                let () = msg_send![view, setKeyEquivalent: ns_string(key)];
                let tint: Id = match variant.as_str() {
                    "destructive" | "danger" => color("red"),
                    _ => ptr::null_mut(),
                };
                let () = msg_send![view, setBezelColor: tint];
                let destructive = matches!(variant.as_str(), "destructive" | "danger");
                let () = msg_send![view, setHasDestructiveAction: destructive as BOOL];
                let tint: Id = match variant.as_str() {
                    "link" => color("accent"),
                    "plain" => color("label"),
                    _ => ptr::null_mut(),
                };
                let () = msg_send![view, setContentTintColor: tint];
                app.layouts.mark_dirty(handle);
                relayout = true;
            }
            (
                Kind::Button
                | Kind::Select
                | Kind::Segmented
                | Kind::Slider
                | Kind::Checkbox
                | Kind::Toggle
                | Kind::TextInput
                | Kind::SecureInput,
                "controlSize",
            ) => {
                let size: usize = match text(&value).as_str() {
                    "sm" | "small" => 1,
                    "xs" | "mini" => 2,
                    "lg" | "large" => 3,
                    _ => 0,
                };
                let () = msg_send![view, setControlSize: size];
                app.layouts.mark_dirty(handle);
                relayout = true;
            }
            (Kind::Button, "icon") => {
                let name = text(&value);
                let image = if name.is_empty() {
                    ptr::null_mut()
                } else {
                    image_for(&name)
                };
                let () = msg_send![view, setImage: image];
                let () = msg_send![view, setImagePosition: 7usize]; // leading
                release(image);
                app.layouts.mark_dirty(handle);
                relayout = true;
            }

            // Inputs
            (Kind::TextInput | Kind::SecureInput, "value") => {
                let current: Id = msg_send![view, stringValue];
                let next = text(&value);
                if rust_string(current) != next {
                    let () = msg_send![view, setStringValue: ns_string(&next)];
                }
            }
            (Kind::TextInput | Kind::SecureInput, "placeholder") => {
                let () = msg_send![view, setPlaceholderString: ns_string(&text(&value))];
            }
            (Kind::TextArea, "value") => {
                let current: Id = msg_send![inner, string];
                let next = text(&value);
                if rust_string(current) != next {
                    let () = msg_send![inner, setString: ns_string(&next)];
                }
            }

            // Ranges
            (Kind::Slider | Kind::Progress, "value") => {
                let () = msg_send![view, setDoubleValue: num(&value)];
            }
            (Kind::Slider | Kind::Progress, "min") => {
                let () = msg_send![view, setMinValue: num(&value)];
            }
            (Kind::Slider | Kind::Progress, "max") => {
                let () = msg_send![view, setMaxValue: num(&value)];
            }
            (Kind::Progress, "indeterminate") => {
                let on = truthy(&value);
                let () = msg_send![view, setIndeterminate: on as BOOL];
                if on {
                    let () = msg_send![view, startAnimation: ptr::null_mut::<Object>()];
                } else {
                    let () = msg_send![view, stopAnimation: ptr::null_mut::<Object>()];
                }
            }

            // Choices
            (Kind::Toggle | Kind::Checkbox, "checked") => {
                let () = msg_send![view, setState: if truthy(&value) { 1isize } else { 0isize }];
            }
            (Kind::Checkbox, "label") => {
                let () = msg_send![view, setTitle: ns_string(&text(&value))];
                app.layouts.mark_dirty(handle);
                relayout = true;
            }
            (Kind::Select, "options") => {
                let () = msg_send![view, removeAllItems];
                for item in string_list(&value) {
                    let () = msg_send![view, addItemWithTitle: ns_string(&item)];
                }
                app.layouts.mark_dirty(handle);
                relayout = true;
            }
            (Kind::Select, "selectedIndex") => {
                let () = msg_send![view, selectItemAtIndex: num(&value) as isize];
            }
            (Kind::Segmented, "options") => {
                let items = string_list(&value);
                let () = msg_send![view, setSegmentCount: items.len() as isize];
                for (i, item) in items.iter().enumerate() {
                    let () = msg_send![view, setLabel: ns_string(item) forSegment: i as isize];
                    let () = msg_send![view, setWidth: 0.0f64 forSegment: i as isize];
                }
                app.layouts.mark_dirty(handle);
                relayout = true;
            }
            (Kind::Segmented, "selectedIndex") => {
                let () = msg_send![view, setSelectedSegment: num(&value) as isize];
            }

            // Images
            (Kind::Image, "src" | "symbol" | "name") => {
                let name = text(&value);
                let image = if name.is_empty() {
                    ptr::null_mut()
                } else {
                    image_for(&name)
                };
                let () = msg_send![view, setImage: image];
                release(image);
                app.layouts.mark_dirty(handle);
                relayout = true;
            }
            (Kind::Image, "color") => {
                let () = msg_send![view, setContentTintColor: color(&text(&value))];
            }
            (Kind::Image, "size") => {
                let config: Id = msg_send![class!(NSImageSymbolConfiguration),
                    configurationWithPointSize: num(&value) weight: 0.0f64];
                let () = msg_send![view, setSymbolConfiguration: config];
                app.layouts.mark_dirty(handle);
                relayout = true;
            }
            _ => {}
        }
    }
    if relayout {
        invalidate(app, handle);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Tree

pub fn insert(parent: u32, child: u32, before: u32) -> Result<(), String> {
    with_app(|app| {
        autoreleasepool(|| unsafe {
            let parent_widget = app
                .widgets
                .get(&parent)
                .ok_or_else(|| format!("no widget {parent}"))?;
            if !parent_widget.kind.holds_children() {
                return Err(format!("{:?} cannot hold children", parent_widget.kind));
            }
            let (parent_kind, parent_view, parent_doc) =
                (parent_widget.kind, parent_widget.view, parent_widget.inner);
            let child_widget = app
                .widgets
                .get_mut(&child)
                .ok_or_else(|| format!("no widget {child}"))?;
            if child_widget.kind == Kind::Window {
                return Err("a window cannot be placed inside another widget".into());
            }
            child_widget.parent = parent;
            let child_view = child_widget.view;
            let host = if parent_kind == Kind::Scroll {
                parent_doc
            } else {
                parent_view
            };
            let () = msg_send![child_view, removeFromSuperview];
            let () = msg_send![host, addSubview: child_view];
            let layout_parent = if parent_kind == Kind::Scroll {
                parent | SCROLL_DOCUMENT_BIT
            } else {
                parent
            };
            let before = (before != 0).then_some(before);
            app.layouts.insert(layout_parent, child, before);
            invalidate(app, parent);
            Ok(())
        })
    })
}

pub fn remove(parent: u32, child: u32) -> Result<(), String> {
    with_app(|app| {
        autoreleasepool(|| unsafe {
            let parent_kind = app.widgets.get(&parent).map(|w| w.kind);
            let layout_parent = if parent_kind == Some(Kind::Scroll) {
                parent | SCROLL_DOCUMENT_BIT
            } else {
                parent
            };
            app.layouts.remove(layout_parent, child);
            if let Some(widget) = app.widgets.get_mut(&child) {
                widget.parent = 0;
                let () = msg_send![widget.view, removeFromSuperview];
            }
            invalidate(app, parent);
            Ok(())
        })
    })
}

pub fn destroy(handle: u32) -> Result<(), String> {
    let window = with_app(|app| {
        autoreleasepool(|| unsafe {
            let Some(widget) = app.widgets.remove(&handle) else {
                return Ok(None);
            };
            if widget.parent != 0 {
                invalidate(app, widget.parent);
            }
            app.layouts.drop_node(handle);
            if widget.kind == Kind::Scroll {
                app.layouts.drop_node(handle | SCROLL_DOCUMENT_BIT);
            }
            REGISTRY.with(|r| {
                let mut r = r.borrow_mut();
                r.remove(&(widget.view as usize));
                if !widget.inner.is_null() {
                    r.remove(&(widget.inner as usize));
                }
            });
            PAINTS.with(|p| p.borrow_mut().remove(&(widget.view as usize)));
            if widget.kind == Kind::Window {
                ROOTS.with(|r| r.borrow_mut().remove(&(widget.view as usize)));
                // Closing calls the delegate; do it after the state is released.
                return Ok(Some((widget.inner, widget.view)));
            }
            let () = msg_send![widget.view, removeFromSuperview];
            release(widget.view);
            release(widget.inner);
            Ok(None)
        })
    })?;
    if let Some((window, root)) = window {
        autoreleasepool(|| unsafe {
            let () = msg_send![window, setDelegate: ptr::null_mut::<Object>()];
            let () = msg_send![window, close];
            release(root);
            release(window);
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Commands

/// `show`, `hide`, `close`, `minimize`, `maximize`, `unmaximize`, `fullscreen` (arg "true" or
/// "false"), `center`, `focus` and `snapshot` (arg: a PNG path) for windows; `click` and `focus`
/// for controls.
/// What AppKit holds for `handle`'s assistive technology, as JSON: `label`, `hint`, `element`
/// (whether it is announced at all) and `focused`. The role is kiln's, from the widget kind.
pub fn describe(handle: u32) -> Result<String, String> {
    let (kind, view, inner) = with_app(|app| {
        let widget = app
            .widgets
            .get(&handle)
            .ok_or_else(|| format!("no widget {handle}"))?;
        Ok((widget.kind, widget.view, widget.inner))
    })?;
    autoreleasepool(|| unsafe {
        let target = if kind == Kind::TextArea { inner } else { view };
        let label: Id = msg_send![target, accessibilityLabel];
        let hint: Id = msg_send![target, accessibilityHelp];
        let element: BOOL = msg_send![target, isAccessibilityElement];
        let window: Id = msg_send![view, window];
        let focused = !window.is_null() && {
            let responder: Id = msg_send![window, firstResponder];
            // A text field being edited hands first responder to its field editor, whose
            // delegate is the field.
            let field_editor: BOOL = msg_send![responder, isKindOfClass: class!(NSTextView)];
            let delegate: Id = if field_editor == YES {
                msg_send![responder, delegate]
            } else {
                ptr::null_mut()
            };
            responder == target || delegate == target
        };
        Ok(serde_json::json!({
            "label": rust_string(label),
            "hint": rust_string(hint),
            "element": element == YES,
            "focused": focused,
        })
        .to_string())
    })
}

pub fn call(handle: u32, command: &str, arg: &str) -> Result<i32, String> {
    let (kind, view, window, activate) = with_app(|app| {
        let widget = app
            .widgets
            .get(&handle)
            .ok_or_else(|| format!("no widget {handle}"))?;
        let activate = command == "show" && !app.activated;
        if activate {
            app.activated = true;
        }
        Ok((widget.kind, widget.view, widget.inner, activate))
    })?;
    autoreleasepool(|| unsafe {
        if kind != Kind::Window {
            return match command {
                // A real click: AppKit runs the control's action, which queues the event.
                "click" => {
                    let () = msg_send![view, performClick: ptr::null_mut::<Object>()];
                    Ok(0)
                }
                "focus" => {
                    let win: Id = msg_send![view, window];
                    let target = if kind == Kind::TextArea { window } else { view };
                    if !win.is_null() {
                        let () = msg_send![win, makeFirstResponder: target];
                    }
                    Ok(0)
                }
                _ => Err(format!("{kind:?} has no command `{command}`")),
            };
        }
        match command {
            "show" => {
                let () = msg_send![window, makeKeyAndOrderFront: ptr::null_mut::<Object>()];
                if activate {
                    let app: Id = msg_send![class!(NSApplication), sharedApplication];
                    let () = msg_send![app, activateIgnoringOtherApps: YES];
                }
                let () = msg_send![view, setNeedsLayout: YES];
            }
            "hide" => {
                let () = msg_send![window, orderOut: ptr::null_mut::<Object>()];
            }
            "close" => {
                let () = msg_send![window, close];
            }
            "minimize" => {
                let () = msg_send![window, miniaturize: ptr::null_mut::<Object>()];
            }
            "maximize" | "unmaximize" => {
                let zoomed: BOOL = msg_send![window, isZoomed];
                if (zoomed == YES) != (command == "maximize") {
                    let () = msg_send![window, zoom: ptr::null_mut::<Object>()];
                }
            }
            "fullscreen" => {
                let mask: usize = msg_send![window, styleMask];
                let is_full = mask & (1 << 14) != 0;
                if is_full != (arg == "true") {
                    let () = msg_send![window, toggleFullScreen: ptr::null_mut::<Object>()];
                }
            }
            "center" => {
                let () = msg_send![window, center];
            }
            "focus" => {
                let () = msg_send![window, makeKeyAndOrderFront: ptr::null_mut::<Object>()];
            }
            "snapshot" => {
                let () = msg_send![view, layoutSubtreeIfNeeded];
                let () = msg_send![view, displayIfNeeded];
                let content: Id = msg_send![window, contentView];
                let bounds: CGRect = msg_send![content, bounds];
                let rep: Id = msg_send![content, bitmapImageRepForCachingDisplayInRect: bounds];
                if rep.is_null() {
                    return Err("could not capture the window".into());
                }
                let () = msg_send![content, cacheDisplayInRect: bounds toBitmapImageRep: rep];
                let props: Id = msg_send![class!(NSDictionary), dictionary];
                let png: Id = msg_send![rep, representationUsingType: 4usize properties: props];
                let ok: BOOL = msg_send![png, writeToFile: ns_string(arg) atomically: YES];
                if ok != YES {
                    return Err(format!("could not write {arg}"));
                }
            }
            other => return Err(format!("windows have no command `{other}`")),
        }
        Ok(0)
    })
}

// ---------------------------------------------------------------------------------------------
// Events

/// Ends a running [`wait`] early. Safe from any thread: it posts an application-defined event,
/// which the main thread's event wait receives.
pub fn wake() {
    autoreleasepool(|| unsafe {
        let event: Id = msg_send![class!(NSEvent),
            otherEventWithType: 15usize // NSEventTypeApplicationDefined
            location: CGPoint::default()
            modifierFlags: 0usize
            timestamp: 0.0f64
            windowNumber: 0isize
            context: ptr::null_mut::<Object>()
            subtype: 0i16
            data1: 0isize
            data2: 0isize];
        let app: Id = msg_send![class!(NSApplication), sharedApplication];
        let () = msg_send![app, postEvent: event atStart: NO];
    });
}

/// Waits up to `timeout_ms` (negative: until an event) for the first OS event, then dispatches
/// every queued one. Returns whether the kiln event queue holds anything for the program.
pub fn wait(timeout_ms: f64) -> Result<bool, String> {
    if APP.with(|cell| cell.borrow().is_none()) {
        return Err("kiln:gui is not initialised on this thread".into());
    }
    flush_dirty_windows();
    autoreleasepool(|| unsafe {
        let app: Id = msg_send![class!(NSApplication), sharedApplication];
        let mode: Id = ns_string("kCFRunLoopDefaultMode");
        let mut until: Id = if !events::is_empty() || timeout_ms == 0.0 {
            msg_send![class!(NSDate), distantPast]
        } else if timeout_ms < 0.0 {
            msg_send![class!(NSDate), distantFuture]
        } else {
            msg_send![class!(NSDate), dateWithTimeIntervalSinceNow: timeout_ms / 1000.0]
        };
        loop {
            let event: Id = msg_send![app, nextEventMatchingMask: EVENT_MASK_ANY
                                                      untilDate: until
                                                         inMode: mode
                                                        dequeue: YES];
            if event.is_null() {
                break;
            }
            let () = msg_send![app, sendEvent: event];
            until = msg_send![class!(NSDate), distantPast];
        }
        let () = msg_send![app, updateWindows];
    });
    Ok(!events::is_empty())
}

// ---------------------------------------------------------------------------------------------
// Menus

/// Builds the menu bar: the application menu, Edit (so text fields copy and paste), the
/// program's own menus, then Window.
fn install_menu(app: Id, target: Id, menus: &[Value]) {
    unsafe {
        let bar: Id = msg_send![class!(NSMenu), new];
        let process: Id = msg_send![class!(NSProcessInfo), processInfo];
        let name = rust_string(msg_send![process, processName]);

        let app_menu = submenu(bar, &name);
        add_item(
            app_menu,
            &format!("About {name}"),
            sel!(orderFrontStandardAboutPanel:),
            "",
            ptr::null_mut(),
        );
        separator(app_menu);
        add_item(
            app_menu,
            &format!("Hide {name}"),
            sel!(hide:),
            "h",
            ptr::null_mut(),
        );
        let others = add_item(
            app_menu,
            "Hide Others",
            sel!(hideOtherApplications:),
            "h",
            ptr::null_mut(),
        );
        let () = msg_send![others, setKeyEquivalentModifierMask: (1usize << 20) | (1usize << 19)];
        add_item(
            app_menu,
            "Show All",
            sel!(unhideAllApplications:),
            "",
            ptr::null_mut(),
        );
        separator(app_menu);
        add_item(
            app_menu,
            &format!("Quit {name}"),
            sel!(kilnQuit:),
            "q",
            target,
        );

        let edit = submenu(bar, "Edit");
        add_item(edit, "Undo", sel!(undo:), "z", ptr::null_mut());
        add_item(edit, "Redo", sel!(redo:), "Z", ptr::null_mut());
        separator(edit);
        add_item(edit, "Cut", sel!(cut:), "x", ptr::null_mut());
        add_item(edit, "Copy", sel!(copy:), "c", ptr::null_mut());
        add_item(edit, "Paste", sel!(paste:), "v", ptr::null_mut());
        add_item(edit, "Select All", sel!(selectAll:), "a", ptr::null_mut());

        MENU_IDS.with(|ids| ids.borrow_mut().clear());
        for menu in menus {
            let title = menu.get("label").map(text).unwrap_or_default();
            let sub = submenu(bar, &title);
            build_items(sub, target, menu.get("items"));
        }

        let window = submenu(bar, "Window");
        add_item(
            window,
            "Minimize",
            sel!(performMiniaturize:),
            "m",
            ptr::null_mut(),
        );
        add_item(window, "Zoom", sel!(performZoom:), "", ptr::null_mut());
        let full = add_item(
            window,
            "Enter Full Screen",
            sel!(toggleFullScreen:),
            "f",
            ptr::null_mut(),
        );
        let () = msg_send![full, setKeyEquivalentModifierMask: (1usize << 20) | (1usize << 18)];
        separator(window);
        add_item(window, "Close", sel!(performClose:), "w", ptr::null_mut());
        let () = msg_send![app, setWindowsMenu: window];

        let () = msg_send![app, setMainMenu: bar];
        let () = msg_send![bar, release];
    }
}

unsafe fn submenu(bar: Id, title: &str) -> Id {
    unsafe {
        let item: Id = msg_send![bar, addItemWithTitle: ns_string(title) action: ptr::null::<Object>() keyEquivalent: ns_string("")];
        let alloc: Id = msg_send![class!(NSMenu), alloc];
        let menu: Id = msg_send![alloc, initWithTitle: ns_string(title)];
        let () = msg_send![item, setSubmenu: menu];
        let () = msg_send![menu, release];
        menu
    }
}

unsafe fn separator(menu: Id) {
    unsafe {
        let item: Id = msg_send![class!(NSMenuItem), separatorItem];
        let () = msg_send![menu, addItem: item];
    }
}

unsafe fn add_item(menu: Id, title: &str, action: Sel, key: &str, target: Id) -> Id {
    unsafe {
        let item: Id = msg_send![menu, addItemWithTitle: ns_string(title) action: action keyEquivalent: ns_string(key)];
        if !target.is_null() {
            let () = msg_send![item, setTarget: target];
        }
        item
    }
}

/// `"CmdOrCtrl+Shift+S"` -> ("s", command | shift).
fn accelerator(spec: &str) -> (String, usize) {
    let mut mask = 0usize;
    let mut key = String::new();
    for part in spec.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "cmd" | "command" | "cmdorctrl" | "commandorcontrol" | "super" | "meta" => {
                mask |= 1 << 20
            }
            "ctrl" | "control" => mask |= 1 << 18,
            "alt" | "option" | "opt" => mask |= 1 << 19,
            "shift" => mask |= 1 << 17,
            "enter" | "return" => key = "\r".into(),
            "tab" => key = "\t".into(),
            "space" => key = " ".into(),
            "backspace" | "delete" => key = "\u{8}".into(),
            "escape" | "esc" => key = "\u{1b}".into(),
            other => key = other.to_string(),
        }
    }
    (key, mask)
}

unsafe fn build_items(menu: Id, target: Id, items: Option<&Value>) {
    let Some(Value::Array(items)) = items else {
        return;
    };
    for item in items {
        unsafe {
            if item.get("separator").is_some_and(truthy) {
                separator(menu);
                continue;
            }
            let label = item.get("label").map(text).unwrap_or_default();
            if let Some(children) = item.get("items") {
                let sub = submenu(menu, &label);
                build_items(sub, target, Some(children));
                continue;
            }
            let (key, mask) = item
                .get("accelerator")
                .map(|a| accelerator(&text(a)))
                .unwrap_or_default();
            let entry: Id = msg_send![menu, addItemWithTitle: ns_string(&label) action: sel!(kilnMenu:) keyEquivalent: ns_string(&key)];
            if mask != 0 {
                let () = msg_send![entry, setKeyEquivalentModifierMask: mask];
            }
            let () = msg_send![entry, setTarget: target];
            let tag = MENU_IDS.with(|ids| {
                let mut ids = ids.borrow_mut();
                ids.push(item.get("id").map(text).unwrap_or_else(|| label.clone()));
                ids.len() - 1
            });
            let () = msg_send![entry, setTag: tag as isize];
            if item.get("enabled").is_some_and(|e| !truthy(e)) {
                let () = msg_send![menu, setAutoenablesItems: NO];
                let () = msg_send![entry, setEnabled: NO];
            }
            if item.get("checked").is_some_and(truthy) {
                let () = msg_send![entry, setState: 1isize];
            }
        }
    }
}

/// Replaces the program's menus (between Edit and Window) with `spec`, a JSON array of
/// `{ label, items: [{ id, label, accelerator, enabled, checked, items } | { separator: true }] }`.
pub fn set_menu(spec: &str) -> Result<(), String> {
    let menus: Value = serde_json::from_str(spec).map_err(|e| format!("menu: {e}"))?;
    let menus = menus.as_array().cloned().unwrap_or_default();
    let target = with_app(|app| Ok(app.target))?;
    autoreleasepool(|| unsafe {
        let app: Id = msg_send![class!(NSApplication), sharedApplication];
        install_menu(app, target, &menus);
    });
    Ok(())
}
