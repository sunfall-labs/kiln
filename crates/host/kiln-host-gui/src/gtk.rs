//! The GTK 4 backend of kiln:gui (ADR 0074), for Linux.
//!
//! It mirrors the AppKit backend (`appkit.rs`) property by property. Every widget is a GTK widget
//! owned by this module and named by a `u32` handle that this module allocates. All state is
//! thread-local and initialisation refuses any thread but the process's main one: GTK is a
//! single-threaded toolkit, and the default GLib main context belongs to that thread.
//!
//! Layout is one model for every container: Taffy computes frames from the style object the
//! TypeScript side sends (`layout`), and GTK only measures leaves (`gtk_widget_measure`). Every
//! container — a window's root, a stack, a card, a scroll view's document — is a [`KilnBox`], a
//! widget that positions its children itself: its `size_allocate` allocates each child at the
//! frame Taffy computed. A window's root runs the layout from its own `size_allocate`, which GTK
//! calls during its layout phase, so live resizing re-flows without a round trip through
//! JavaScript. A scroll view's document is its own layout root: it reports its content height
//! from `measure` (height-for-width) and lays its children out at the width it is given.
//!
//! Re-entrancy: GTK calls back into this module (signal handlers, `measure`, `size_allocate`,
//! `snapshot`). Signal handlers only push to the event queue, never touch [`APP`], and are
//! silenced while an operation of ours runs (a programmatic `set_text` emits `changed` exactly
//! like typing does; AppKit's actions fire only for the person). `measure` and `size_allocate`
//! take the state with `try_borrow_mut`/`try_borrow` and fall back to the last frames they placed
//! when an operation holds it.

#![cfg(all(target_os = "linux", feature = "gtk"))]

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, gio, glib, graphene, gsk, pango};
use serde_json::Value;
use taffy::prelude::{AvailableSpace, Size as TaffySize};

use crate::events::{self, EventKind};
use crate::layout::{Frame, Layouts, parse_style};

const SCROLL_DOCUMENT_BIT: u32 = 0x8000_0000;

/// What kind of native widget a handle names. Parsed once from the tag the TypeScript side sends.
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
    widget: gtk::Widget,
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

#[derive(Clone, Default)]
struct Paint {
    background: Option<String>,
    border_color: Option<String>,
    border_width: f64,
    corner_radius: f64,
}

/// The parts of a window beside its root container.
struct WindowParts {
    window: gtk::Window,
    menubar: gtk::PopoverMenuBar,
    shortcuts: gtk::ShortcutController,
    /// The size the program last asked for, until the window reports its own.
    size: (i32, i32),
}

struct Widget {
    kind: Kind,
    /// The widget placed in the parent (a window's root container for a window).
    view: gtk::Widget,
    /// The text view of a text area; the document of a scroll view.
    inner: Option<gtk::Widget>,
    window: Option<WindowParts>,
    parent: u32,
    text: TextStyle,
    /// The CSS class that colors this widget's text or symbol, if any.
    color_class: Option<String>,
    /// What assistive technology is told (GTK has no getters for these).
    label: String,
    hint: String,
    pressable: Option<gtk::GestureClick>,
    /// A button's title and icon, rebuilt together.
    button: (String, String),
    /// A range's minimum, maximum and value (GtkProgressBar only knows a fraction).
    range: (f64, f64, f64),
    pulse: Option<glib::SourceId>,
    segments: Vec<gtk::ToggleButton>,
    options: Vec<String>,
}

struct App {
    next_handle: u32,
    widgets: HashMap<u32, Widget>,
    layouts: Layouts<Leaf>,
    /// Windows whose layout must run before the next frame.
    dirty: HashSet<u32>,
    menus: Vec<Value>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// GTK widget pointer -> handle, for `size_allocate`, which must not need [`APP`].
    static REGISTRY: RefCell<HashMap<usize, u32>> = RefCell::new(HashMap::new());
    /// GTK widget pointer -> the frame it was last allocated, for passes that cannot read [`APP`].
    static PLACED: RefCell<HashMap<usize, Frame>> = RefCell::new(HashMap::new());
    /// Above zero while an operation of ours runs: signals it causes are not the person's.
    static QUIET: Cell<u32> = const { Cell::new(0) };
    static CSS: RefCell<Css> = RefCell::new(Css::default());
    static DARK: Cell<bool> = const { Cell::new(false) };
}

/// Set by [`wake`] from any thread; [`wait`] returns when it sees it.
static WOKEN: AtomicBool = AtomicBool::new(false);

fn key(widget: &impl IsA<gtk::Widget>) -> usize {
    widget.as_ref().as_ptr() as usize
}

fn lookup(widget: &gtk::Widget) -> Option<u32> {
    REGISTRY.with(|r| r.borrow().get(&key(widget)).copied())
}

struct Quiet;

impl Quiet {
    fn new() -> Quiet {
        QUIET.with(|q| q.set(q.get() + 1));
        Quiet
    }
}

impl Drop for Quiet {
    fn drop(&mut self) {
        QUIET.with(|q| q.set(q.get().saturating_sub(1)));
    }
}

fn with_app<R>(f: impl FnOnce(&mut App) -> Result<R, String>) -> Result<R, String> {
    let result = APP.with(|cell| {
        let mut guard = cell
            .try_borrow_mut()
            .map_err(|_| "kiln:gui re-entered the GTK backend during an operation".to_string())?;
        let app = guard
            .as_mut()
            .ok_or_else(|| "kiln:gui is not initialised on this thread".to_string())?;
        let _quiet = Quiet::new();
        f(app)
    });
    flush_dirty_windows();
    result
}

fn push(handle: u32, kind: EventKind, a: f64, b: f64, text: String) {
    if QUIET.with(Cell::get) > 0 {
        return;
    }
    events::push(events::Event {
        handle,
        kind,
        a,
        b,
        text,
    });
}

fn is_main_thread() -> bool {
    // SAFETY: both calls only read the calling thread's and process's ids.
    unsafe { libc::syscall(libc::SYS_gettid) as libc::pid_t == libc::getpid() }
}

// ---------------------------------------------------------------------------------------------
// Colors, fonts and icons

fn parse_hex(hex: &str) -> Option<gdk::RGBA> {
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
        _ => return None,
    };
    let f = |v: u8| v as f32 / 255.0;
    Some(gdk::RGBA::new(f(r), f(g), f(b), f(a)))
}

/// A semantic name (`"secondary"`, `"accent"`, `"red"`), `"#rgb"`, `"#rrggbb"` or `"#rrggbbaa"`,
/// resolved against the GNOME palette for the current light or dark appearance.
fn rgba(name: &str) -> gdk::RGBA {
    let name = name.trim();
    if let Some(hex) = name.strip_prefix('#') {
        return parse_hex(hex).unwrap_or(gdk::RGBA::BLACK);
    }
    let dark = DARK.with(Cell::get);
    let fg = |alpha: f32| {
        if dark {
            gdk::RGBA::new(1.0, 1.0, 1.0, alpha)
        } else {
            gdk::RGBA::new(0.0, 0.0, 0.0, alpha * 0.8)
        }
    };
    let hex = |light: &str, dark_hex: &str| {
        parse_hex(if dark { dark_hex } else { light }).unwrap_or(gdk::RGBA::BLACK)
    };
    match name {
        "label" | "primary" | "text" | "default" => fg(1.0),
        "secondary" => fg(0.55),
        "tertiary" => fg(0.3),
        "quaternary" => fg(0.15),
        "accent" | "tint" | "selection" => hex("3584e4", "3584e4"),
        "separator" => fg(0.15),
        "background" | "window" => hex("fafafa", "242424"),
        "control" | "content" => hex("ffffff", "1e1e1e"),
        "fill" | "card" => fg(0.07),
        "link" => hex("1c71d8", "78aeed"),
        "red" | "danger" | "error" | "destructive" => hex("e01b24", "ff7b63"),
        "orange" | "warning" => hex("ff7800", "ffa348"),
        "yellow" => hex("f6d32d", "f8e45c"),
        "green" | "success" => hex("26a269", "8ff0a4"),
        "mint" => hex("2ec27e", "57e389"),
        "teal" => hex("2190a4", "5fc8d6"),
        "cyan" => hex("00a3c4", "62d0e8"),
        "blue" | "info" => hex("3584e4", "62a0ea"),
        "indigo" => hex("5e5ce6", "8f8dfa"),
        "purple" => hex("9141ac", "c061cb"),
        "pink" => hex("e5508d", "f37eae"),
        "brown" => hex("986a44", "cdab8f"),
        "gray" | "grey" => hex("77767b", "9a9996"),
        "white" => gdk::RGBA::WHITE,
        "black" => gdk::RGBA::BLACK,
        "clear" | "transparent" => gdk::RGBA::TRANSPARENT,
        _ => fg(1.0),
    }
}

/// One stylesheet for the whole process: the fixed rules below plus one `kiln-c-…` class per
/// color a widget has asked for, added as they are first used.
#[derive(Default)]
struct Css {
    provider: Option<gtk::CssProvider>,
    rules: String,
    known: HashSet<String>,
}

fn base_css() -> String {
    let separator = rgba("separator").to_str();
    let control = rgba("control").to_str();
    format!(
        "
.kiln-textarea {{ border: 1px solid {separator}; border-radius: 6px; background: {control}; }}
.kiln-textarea textview, .kiln-textarea text {{ background: transparent; }}
scrolledwindow.kiln-scroll, scrolledwindow.kiln-scroll > viewport {{ background: transparent; }}
.kiln-size-small {{ min-height: 22px; padding: 1px 8px; font-size: 0.9em; }}
.kiln-size-mini {{ min-height: 18px; padding: 0 6px; font-size: 0.8em; }}
.kiln-size-large {{ min-height: 40px; padding: 6px 16px; font-size: 1.1em; }}
"
    )
}

fn install_css() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    CSS.with(|css| {
        let mut css = css.borrow_mut();
        let provider = gtk::CssProvider::new();
        css.rules = base_css();
        #[allow(deprecated)]
        provider.load_from_data(&css.rules);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        css.provider = Some(provider);
    });
}

/// The CSS class that sets `color` to `name`.
fn color_class(name: &str) -> String {
    let c = rgba(name);
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let class = format!(
        "kiln-c-{:02x}{:02x}{:02x}{:02x}",
        byte(c.red()),
        byte(c.green()),
        byte(c.blue()),
        byte(c.alpha())
    );
    CSS.with(|css| {
        let mut css = css.borrow_mut();
        if css.known.insert(class.clone()) {
            let rule = format!(".{class} {{ color: {}; }}\n", c.to_str());
            css.rules.push_str(&rule);
            if let Some(provider) = &css.provider {
                #[allow(deprecated)]
                provider.load_from_data(&css.rules);
            }
        }
    });
    class
}

fn set_color(widget: &mut Widget, target: &gtk::Widget, name: &str) {
    if let Some(old) = widget.color_class.take() {
        target.remove_css_class(&old);
    }
    if !name.trim().is_empty() {
        let class = color_class(name);
        target.add_css_class(&class);
        widget.color_class = Some(class);
    }
}

/// freedesktop icon names for the SF Symbol names (and short aliases) the AppKit backend takes;
/// anything else is looked up as given.
fn icon_name(name: &str) -> String {
    let mapped = match name {
        "settings" | "gear" | "gearshape" | "gearshape.fill" => "emblem-system-symbolic",
        "search" | "magnifyingglass" => "system-search-symbolic",
        "play" | "play.fill" => "media-playback-start-symbolic",
        "pause" | "pause.fill" => "media-playback-pause-symbolic",
        "stop" | "stop.fill" => "media-playback-stop-symbolic",
        "refresh" | "reload" | "arrow.clockwise" => "view-refresh-symbolic",
        "terminal" | "terminal.fill" => "utilities-terminal-symbolic",
        "memory" | "memorychip" => "drive-harddisk-symbolic",
        "check" | "checkmark" | "success" | "checkmark.circle.fill" => "object-select-symbolic",
        "error" | "danger" | "warning" | "exclamationmark.triangle.fill" => {
            "dialog-warning-symbolic"
        }
        "info" | "info.circle.fill" => "dialog-information-symbolic",
        "folder" | "folder.fill" => "folder-symbolic",
        "file" | "doc" | "doc.text" | "doc.text.fill" => "text-x-generic-symbolic",
        "doc.on.doc" | "copy" => "edit-copy-symbolic",
        "add" | "plus" => "list-add-symbolic",
        "remove" | "minus" => "list-remove-symbolic",
        "delete" | "trash" | "trash.fill" => "user-trash-symbolic",
        "close" | "xmark" => "window-close-symbolic",
        "edit" | "pencil" => "document-edit-symbolic",
        "star" | "star.fill" => "starred-symbolic",
        "heart" | "heart.fill" => "emblem-favorite-symbolic",
        "user" | "person" | "person.crop.circle" => "avatar-default-symbolic",
        "circle" => "radio-symbolic",
        "record.circle" | "record.circle.fill" | "circle.fill" => "media-record-symbolic",
        "note.text" | "note" => "accessories-text-editor-symbolic",
        "slider.horizontal.3" => "preferences-system-symbolic",
        "gauge.with.dots.needle.33percent" | "gauge" | "speedometer" => {
            "power-profile-balanced-symbolic"
        }
        other => other,
    };
    let Some(display) = gdk::Display::default() else {
        return mapped.to_string();
    };
    let theme = gtk::IconTheme::for_display(&display);
    if theme.has_icon(mapped) {
        mapped.to_string()
    } else if theme.has_icon(name) {
        name.to_string()
    } else if !name.ends_with("-symbolic") && theme.has_icon(&format!("{name}-symbolic")) {
        format!("{name}-symbolic")
    } else {
        mapped.to_string()
    }
}

fn is_path(name: &str) -> bool {
    name.starts_with('/')
        || name.starts_with("./")
        || name.starts_with("../")
        || name.starts_with('~')
}

fn expand_home(path: &str) -> String {
    match path.strip_prefix('~') {
        Some(rest) => dirs::home_dir()
            .map(|home| format!("{}{rest}", home.to_string_lossy()))
            .unwrap_or_else(|| path.to_string()),
        None => path.to_string(),
    }
}

fn pango_weight(name: &str) -> pango::Weight {
    match name {
        "ultralight" => pango::Weight::Ultralight,
        "thin" => pango::Weight::Thin,
        "light" => pango::Weight::Light,
        "medium" => pango::Weight::Medium,
        "semibold" => pango::Weight::Semibold,
        "bold" => pango::Weight::Bold,
        "heavy" => pango::Weight::Heavy,
        "black" => pango::Weight::Ultraheavy,
        _ => pango::Weight::Normal,
    }
}

/// The AppKit backend's type ramp, in pixels (a GTK pixel is an AppKit point).
fn font_attrs(style: &TextStyle) -> pango::AttrList {
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
    let weight = if style.weight.is_empty() {
        weight
    } else {
        &style.weight
    };
    let attrs = pango::AttrList::new();
    attrs.insert(pango::AttrSize::new_size_absolute(
        (size * pango::SCALE as f64).round() as i32,
    ));
    attrs.insert(pango::AttrInt::new_weight(pango_weight(weight)));
    if style.monospace {
        attrs.insert(pango::AttrString::new_family("monospace"));
    }
    attrs
}

// ---------------------------------------------------------------------------------------------
// KilnBox: the container whose children Taffy places.

const ROLE_CONTAINER: u8 = 0;
const ROLE_ROOT: u8 = 1;
const ROLE_DOCUMENT: u8 = 2;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct KilnBox {
        pub(super) handle: Cell<u32>,
        pub(super) role: Cell<u8>,
        pub(super) paint: RefCell<Paint>,
        /// The root's last size (reported as a resize when it changes); a document's last width.
        pub(super) last_size: Cell<(i32, i32)>,
        pub(super) content_height: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for KilnBox {
        const NAME: &'static str = "KilnBox";
        type Type = super::KilnBox;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for KilnBox {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for KilnBox {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            if self.role.get() == ROLE_DOCUMENT && orientation == gtk::Orientation::Vertical {
                let width = if for_size >= 0 {
                    for_size
                } else {
                    self.last_size.get().0
                };
                if let Some(height) = layout_document(self.handle.get(), width) {
                    self.content_height.set(height.ceil() as i32);
                }
                let height = self.content_height.get();
                return (height, height, -1, -1);
            }
            // Containers are sized by their parent's layout, never by GTK's.
            (0, 0, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let handle = self.handle.get();
            match self.role.get() {
                ROLE_ROOT => {
                    if self.last_size.get() != (width, height) {
                        self.last_size.set((width, height));
                        push(
                            handle,
                            EventKind::Resize,
                            width as f64,
                            height as f64,
                            String::new(),
                        );
                    }
                    layout_root(handle, width, height);
                }
                ROLE_DOCUMENT => {
                    self.last_size.set((width, height));
                    let _ = layout_document(handle, width);
                }
                _ => {}
            }
            allocate_children(self.obj().upcast_ref());
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let paint = self.paint.borrow().clone();
            let (width, height) = (WidgetExt::width(&*obj), WidgetExt::height(&*obj));
            let bounds = graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
            let rounded = gsk::RoundedRect::from_rect(bounds, paint.corner_radius as f32);
            let clip = paint.corner_radius > 0.0;
            if clip {
                snapshot.push_rounded_clip(&rounded);
            }
            if let Some(background) = &paint.background {
                snapshot.append_color(&rgba(background), &bounds);
            }
            self.parent_snapshot(snapshot);
            if clip {
                snapshot.pop();
            }
            if let Some(border) = &paint.border_color
                && paint.border_width > 0.0
            {
                let color = rgba(border);
                let w = paint.border_width as f32;
                snapshot.append_border(&rounded, &[w; 4], &[color; 4]);
            }
        }
    }
}

glib::wrapper! {
    pub struct KilnBox(ObjectSubclass<imp::KilnBox>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl KilnBox {
    fn new(handle: u32, role: u8) -> KilnBox {
        let kbox: KilnBox = glib::Object::new();
        kbox.imp().handle.set(handle);
        kbox.imp().role.set(role);
        kbox
    }
}

// ---------------------------------------------------------------------------------------------
// Layout

fn measure(
    leaf: &mut Leaf,
    known: TaffySize<Option<f32>>,
    available: TaffySize<AvailableSpace>,
) -> TaffySize<f32> {
    let widget = &leaf.widget;
    let (min_width, natural_width, _, _) = widget.measure(gtk::Orientation::Horizontal, -1);
    let width = match known.width {
        Some(w) => w,
        None if leaf.kind == Kind::Text => match available.width {
            // A label takes the width it needs up to what there is, and wraps inside it.
            AvailableSpace::Definite(w) => (natural_width as f32).min(w).max(min_width as f32),
            AvailableSpace::MinContent => min_width as f32,
            AvailableSpace::MaxContent => natural_width as f32,
        },
        None => natural_width as f32,
    };
    let height = match known.height {
        Some(h) => h,
        None => {
            let (_, natural_height, _, _) =
                widget.measure(gtk::Orientation::Vertical, width.ceil() as i32);
            natural_height as f32
        }
    };
    TaffySize {
        width: width.ceil(),
        height: height.ceil(),
    }
}

fn layout_root(window: u32, width: i32, height: i32) {
    APP.with(|cell| {
        if let Ok(mut guard) = cell.try_borrow_mut()
            && let Some(app) = guard.as_mut()
        {
            app.layouts
                .compute(window, Some(width as f64), Some(height as f64), measure);
        }
    });
}

/// Lays out a scroll view's document at `width` and returns its content height.
fn layout_document(document: u32, width: i32) -> Option<f64> {
    APP.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        let app = guard.as_mut()?;
        app.layouts
            .compute(document, Some(width.max(0) as f64), None, measure);
        app.layouts.content_size(document).map(|(_, h)| h)
    })
}

/// Allocates every child of `parent` at the frame the last layout pass gave it.
fn allocate_children(parent: &gtk::Widget) {
    let mut children = Vec::new();
    let mut child = parent.first_child();
    while let Some(c) = child {
        child = c.next_sibling();
        children.push(c);
    }
    let frames: Vec<Option<Frame>> = APP.with(|cell| match cell.try_borrow() {
        Ok(guard) => match guard.as_ref() {
            Some(app) => children
                .iter()
                .map(|c| lookup(c).and_then(|h| app.layouts.frame(h)))
                .collect(),
            None => vec![None; children.len()],
        },
        Err(_) => vec![None; children.len()],
    });
    for (child, frame) in children.iter().zip(frames) {
        let frame = match frame {
            Some(frame) => {
                PLACED.with(|p| p.borrow_mut().insert(key(child), frame));
                frame
            }
            None => PLACED
                .with(|p| p.borrow().get(&key(child)).copied())
                .unwrap_or_default(),
        };
        let x0 = frame.x.round();
        let y0 = frame.y.round();
        let width = ((frame.x + frame.width).round() - x0).max(0.0) as i32;
        let height = ((frame.y + frame.height).round() - y0).max(0.0) as i32;
        // GTK expects a widget to be measured before it is allocated.
        let _ = child.measure(gtk::Orientation::Horizontal, -1);
        let transform =
            gsk::Transform::new().translate(&graphene::Point::new(x0 as f32, y0 as f32));
        child.allocate(width, height, -1, Some(transform));
    }
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

/// Asks GTK to lay out every window an operation touched again, after the operation released
/// the state. Every container is queued, not only the root: GTK skips allocating a child whose
/// own size did not change, but its children's frames may have.
fn flush_dirty_windows() {
    let roots: Vec<gtk::Widget> = APP.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return Vec::new();
        };
        let Some(app) = guard.as_mut() else {
            return Vec::new();
        };
        let dirty: Vec<u32> = app.dirty.drain().collect();
        dirty
            .into_iter()
            .filter_map(|w| app.widgets.get(&w).map(|w| w.view.clone()))
            .collect()
    });
    for root in roots {
        queue_containers(&root);
    }
}

fn queue_containers(widget: &gtk::Widget) {
    if let Some(kbox) = widget.downcast_ref::<KilnBox>() {
        if kbox.imp().role.get() == ROLE_DOCUMENT {
            kbox.queue_resize();
        } else {
            kbox.queue_allocate();
        }
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        queue_containers(&c);
        child = c.next_sibling();
    }
}

// ---------------------------------------------------------------------------------------------
// Lifecycle

pub fn init() -> Result<(), String> {
    if !is_main_thread() {
        return Err("GTK runs only on the main thread, and this realm is not on it".into());
    }
    if APP.with(|cell| cell.borrow().is_some()) {
        return Ok(());
    }
    gtk::init().map_err(|e| {
        format!("GTK could not start ({e}); is there a display (DISPLAY or WAYLAND_DISPLAY)?")
    })?;
    let dark = gtk::Settings::default().is_some_and(|s| {
        s.is_gtk_application_prefer_dark_theme()
            || s.gtk_theme_name()
                .is_some_and(|n| n.to_ascii_lowercase().contains("dark"))
    });
    DARK.with(|d| d.set(dark));
    install_css();
    APP.with(|cell| {
        *cell.borrow_mut() = Some(App {
            next_handle: 1,
            widgets: HashMap::new(),
            layouts: Layouts::new(),
            dirty: HashSet::new(),
            menus: Vec::new(),
        })
    });
    Ok(())
}

pub fn create(tag: &str) -> Result<u32, String> {
    let kind = Kind::parse(tag).ok_or_else(|| format!("GTK has no widget for <{tag}>"))?;
    with_app(|app| create_widget(app, kind))
}

fn create_widget(app: &mut App, kind: Kind) -> Result<u32, String> {
    let handle = app.next_handle;
    app.next_handle += 1;
    let mut inner: Option<gtk::Widget> = None;
    let mut window_parts = None;
    let view: gtk::Widget = match kind {
        Kind::Window => {
            let window = gtk::Window::new();
            window.set_default_size(800, 600);
            // Closing hides; the program decides when a window is gone (`destroy`).
            window.set_hide_on_close(true);
            window.connect_close_request(move |_| {
                push(handle, EventKind::Close, 0.0, 0.0, String::new());
                glib::Propagation::Proceed
            });
            let root = KilnBox::new(handle, ROLE_ROOT);
            root.set_hexpand(true);
            root.set_vexpand(true);
            let menubar = gtk::PopoverMenuBar::from_model(None::<&gio::MenuModel>);
            menubar.set_visible(false);
            let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
            column.append(&menubar);
            column.append(&root);
            window.set_child(Some(&column));
            let shortcuts = gtk::ShortcutController::new();
            window.add_controller(shortcuts.clone());
            let mut parts = WindowParts {
                window,
                menubar,
                shortcuts,
                size: (800, 600),
            };
            install_menu(&mut parts, &app.menus);
            window_parts = Some(parts);
            root.upcast()
        }
        Kind::Container | Kind::Spacer => KilnBox::new(handle, ROLE_CONTAINER).upcast(),
        Kind::Scroll => {
            let scroll = gtk::ScrolledWindow::new();
            scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
            scroll.set_has_frame(false);
            scroll.add_css_class("kiln-scroll");
            let doc = KilnBox::new(handle | SCROLL_DOCUMENT_BIT, ROLE_DOCUMENT);
            scroll.set_child(Some(&doc));
            inner = Some(doc.upcast());
            scroll.upcast()
        }
        Kind::Text => {
            let label = gtk::Label::new(None);
            label.set_wrap(true);
            label.set_wrap_mode(pango::WrapMode::WordChar);
            label.set_xalign(0.0);
            label.set_yalign(0.0);
            label.set_attributes(Some(&font_attrs(&TextStyle::default())));
            label.upcast()
        }
        Kind::Button => {
            let button = gtk::Button::new();
            button
                .connect_clicked(move |_| push(handle, EventKind::Press, 0.0, 0.0, String::new()));
            button.upcast()
        }
        Kind::TextInput | Kind::SecureInput => {
            let entry = gtk::Entry::new();
            if kind == Kind::SecureInput {
                entry.set_visibility(false);
                entry.set_input_purpose(gtk::InputPurpose::Password);
            }
            entry.set_attributes(&font_attrs(&TextStyle::default()));
            entry.connect_changed(move |e| {
                push(handle, EventKind::Input, 0.0, 0.0, e.text().to_string())
            });
            entry.connect_activate(move |e| {
                push(handle, EventKind::Submit, 0.0, 0.0, e.text().to_string())
            });
            entry.upcast()
        }
        Kind::TextArea => {
            let scroll = gtk::ScrolledWindow::new();
            scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
            scroll.add_css_class("kiln-textarea");
            let text_view = gtk::TextView::new();
            text_view.set_wrap_mode(gtk::WrapMode::WordChar);
            text_view.set_left_margin(4);
            text_view.set_right_margin(4);
            text_view.set_top_margin(6);
            text_view.set_bottom_margin(6);
            text_view.buffer().connect_changed(move |buffer| {
                let (start, end) = buffer.bounds();
                push(
                    handle,
                    EventKind::Input,
                    0.0,
                    0.0,
                    buffer.text(&start, &end, false).to_string(),
                )
            });
            scroll.set_child(Some(&text_view));
            REGISTRY.with(|r| r.borrow_mut().insert(key(&text_view), handle));
            inner = Some(text_view.upcast());
            scroll.upcast()
        }
        Kind::Slider => {
            let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
            scale.set_draw_value(false);
            scale.connect_value_changed(move |s| {
                push(handle, EventKind::Change, s.value(), 0.0, String::new())
            });
            scale.upcast()
        }
        Kind::Toggle => {
            let switch = gtk::Switch::new();
            switch.set_halign(gtk::Align::Start);
            switch.set_valign(gtk::Align::Center);
            switch.connect_active_notify(move |s| {
                let on = if s.is_active() { 1.0 } else { 0.0 };
                push(handle, EventKind::Change, on, 0.0, String::new())
            });
            switch.upcast()
        }
        Kind::Checkbox => {
            let check = gtk::CheckButton::new();
            check.set_valign(gtk::Align::Center);
            check.connect_toggled(move |c| {
                let on = if c.is_active() { 1.0 } else { 0.0 };
                push(handle, EventKind::Change, on, 0.0, String::new())
            });
            check.upcast()
        }
        Kind::Select => {
            let dropdown = gtk::DropDown::from_strings(&[]);
            dropdown.set_valign(gtk::Align::Center);
            dropdown.connect_selected_notify(move |d| {
                let index = d.selected();
                let title = d
                    .model()
                    .and_then(|m| m.downcast::<gtk::StringList>().ok())
                    .and_then(|m| m.string(index))
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let index = if index == gtk::INVALID_LIST_POSITION {
                    -1.0
                } else {
                    index as f64
                };
                push(handle, EventKind::Change, index, 0.0, title)
            });
            dropdown.upcast()
        }
        Kind::Segmented => {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            row.add_css_class("linked");
            row.set_valign(gtk::Align::Center);
            row.upcast()
        }
        Kind::Progress => {
            let bar = gtk::ProgressBar::new();
            bar.set_valign(gtk::Align::Center);
            bar.upcast()
        }
        Kind::Spinner => {
            let spinner = gtk::Spinner::new();
            spinner.set_spinning(true);
            spinner.set_halign(gtk::Align::Center);
            spinner.set_valign(gtk::Align::Center);
            spinner.upcast()
        }
        Kind::Image => {
            let image = gtk::Image::new();
            // Decorative until labelled: a screen reader should not announce "image" for an icon.
            image.set_accessible_role(gtk::AccessibleRole::Presentation);
            image.upcast()
        }
        Kind::Divider => gtk::Separator::new(gtk::Orientation::Horizontal).upcast(),
    };
    REGISTRY.with(|r| r.borrow_mut().insert(key(&view), handle));
    let leaf = kind.is_measured().then(|| Leaf {
        widget: view.clone(),
        kind,
    });
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
            window: window_parts,
            parent: 0,
            text: TextStyle::default(),
            color_class: None,
            label: String::new(),
            hint: String::new(),
            pressable: None,
            button: (String::new(), String::new()),
            range: (0.0, 100.0, 0.0),
            pulse: None,
            segments: Vec::new(),
            options: Vec::new(),
        },
    );
    Ok(handle)
}

// ---------------------------------------------------------------------------------------------
// Properties

pub fn set_text(handle: u32, name: &str, value: &str) -> Result<(), String> {
    with_app(|app| apply(app, handle, name, Value::String(value.to_string())))
}

pub fn set_number(handle: u32, name: &str, value: f64) -> Result<(), String> {
    let value = serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null);
    with_app(|app| apply(app, handle, name, value))
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

fn rebuild_button(button: &gtk::Button, title: &str, icon: &str) {
    if icon.is_empty() {
        button.set_label(title);
    } else if title.is_empty() {
        button.set_icon_name(&icon_name(icon));
    } else {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.set_halign(gtk::Align::Center);
        content.append(&gtk::Image::from_icon_name(&icon_name(icon)));
        content.append(&gtk::Label::new(Some(title)));
        button.set_child(Some(&content));
    }
}

fn update_progress(widget: &Widget) {
    let Some(bar) = widget.view.downcast_ref::<gtk::ProgressBar>() else {
        return;
    };
    if widget.pulse.is_some() {
        return;
    }
    let (min, max, value) = widget.range;
    let fraction = if max > min {
        (value - min) / (max - min)
    } else {
        0.0
    };
    bar.set_fraction(fraction.clamp(0.0, 1.0));
}

fn apply(app: &mut App, handle: u32, name: &str, value: Value) -> Result<(), String> {
    let widget = app
        .widgets
        .get_mut(&handle)
        .ok_or_else(|| format!("no widget with handle {handle}"))?;
    let kind = widget.kind;
    let view = widget.view.clone();
    let inner = widget.inner.clone();
    let mut relayout = false;
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
            view.set_visible(!truthy(&value));
            relayout = true;
        }
        (_, "accessibilityLabel" | "accessibilityHint") => {
            let target = if kind == Kind::TextArea {
                inner.clone().unwrap_or_else(|| view.clone())
            } else {
                view.clone()
            };
            let text = text(&value);
            if name == "accessibilityLabel" {
                target.update_property(&[gtk::accessible::Property::Label(&text)]);
                if kind == Kind::Image {
                    // A labelled image is content, not decoration.
                    target.set_accessible_role(if text.is_empty() {
                        gtk::AccessibleRole::Presentation
                    } else {
                        gtk::AccessibleRole::Img
                    });
                }
                widget.label = text;
            } else {
                target.update_property(&[gtk::accessible::Property::Description(&text)]);
                widget.hint = text;
            }
        }
        (_, "tooltip") => {
            let tip = text(&value);
            view.set_tooltip_text((!tip.is_empty()).then_some(tip.as_str()));
        }
        (_, "opacity") => view.set_opacity(num(&value)),
        (
            Kind::Window | Kind::Container | Kind::Spacer,
            "background" | "borderColor" | "borderWidth" | "cornerRadius",
        ) => {
            if let Some(kbox) = view.downcast_ref::<KilnBox>() {
                let mut paint = kbox.imp().paint.borrow_mut();
                match name {
                    "background" => paint.background = Some(text(&value)).filter(|s| !s.is_empty()),
                    "borderColor" => {
                        paint.border_color = Some(text(&value)).filter(|s| !s.is_empty())
                    }
                    "borderWidth" => paint.border_width = num(&value),
                    _ => paint.corner_radius = num(&value),
                }
            }
            view.queue_draw();
        }
        (Kind::Container | Kind::Window, "pressable") => {
            if truthy(&value) && widget.pressable.is_none() {
                let click = gtk::GestureClick::new();
                click.connect_released(move |_, _, _, _| {
                    push(handle, EventKind::Press, 0.0, 0.0, String::new())
                });
                view.add_controller(click.clone());
                view.set_cursor_from_name(Some("pointer"));
                // Settable until assistive technology has realised the widget's context.
                view.set_accessible_role(gtk::AccessibleRole::Button);
                widget.pressable = Some(click);
            } else if !truthy(&value)
                && let Some(click) = widget.pressable.take()
            {
                view.remove_controller(&click);
                view.set_cursor_from_name(None);
            }
        }
        (_, "disabled") => {
            if kind == Kind::TextArea {
                if let Some(text_view) = inner.and_then(|w| w.downcast::<gtk::TextView>().ok()) {
                    text_view.set_editable(!truthy(&value));
                }
            } else {
                view.set_sensitive(!truthy(&value));
            }
        }

        // Window
        (Kind::Window, "title") => {
            if let Some(parts) = &widget.window {
                parts.window.set_title(Some(&text(&value)));
            }
        }
        (Kind::Window, "width" | "height") => {
            if let Some(parts) = &mut widget.window {
                let (mut w, mut h) = if view.is_mapped() {
                    (view.width(), view.height())
                } else {
                    parts.size
                };
                if name == "width" {
                    w = num(&value).round() as i32
                } else {
                    h = num(&value).round() as i32
                }
                parts.size = (w, h);
                parts.window.set_default_size(w, h);
            }
            relayout = true;
        }
        (Kind::Window, "minWidth" | "minHeight") => {
            let (mut w, mut h) = view.size_request();
            if name == "minWidth" {
                w = num(&value).round() as i32
            } else {
                h = num(&value).round() as i32
            }
            view.set_size_request(w, h);
        }
        (Kind::Window, "resizable") => {
            if let Some(parts) = &widget.window {
                parts.window.set_resizable(truthy(&value));
            }
        }

        // Text
        (Kind::Text, "text") => {
            if let Some(label) = view.downcast_ref::<gtk::Label>() {
                label.set_text(&text(&value));
            }
            app.layouts.mark_dirty(handle);
            relayout = true;
        }
        (
            Kind::Text | Kind::TextInput | Kind::SecureInput | Kind::TextArea,
            "variant" | "size" | "weight" | "monospace",
        ) => {
            let style = &mut widget.text;
            match name {
                "variant" => style.variant = text(&value),
                "size" => style.size = num(&value),
                "weight" => style.weight = text(&value),
                _ => style.monospace = truthy(&value),
            }
            let attrs = font_attrs(style);
            if let Some(label) = view.downcast_ref::<gtk::Label>() {
                label.set_attributes(Some(&attrs));
            } else if let Some(entry) = view.downcast_ref::<gtk::Entry>() {
                entry.set_attributes(&attrs);
            } else if let Some(text_view) = inner.and_then(|w| w.downcast::<gtk::TextView>().ok()) {
                text_view.set_monospace(style.monospace);
            }
            app.layouts.mark_dirty(handle);
            relayout = true;
        }
        (Kind::Text | Kind::TextInput | Kind::SecureInput | Kind::Image, "color") => {
            set_color(widget, &view, &text(&value));
        }
        (Kind::TextArea, "color") => {
            if let Some(inner) = inner {
                set_color(widget, &inner, &text(&value));
            }
        }
        (Kind::Text | Kind::TextInput | Kind::SecureInput, "align") => {
            let (x, justify) = match text(&value).as_str() {
                "center" => (0.5, gtk::Justification::Center),
                "end" | "right" | "trailing" => (1.0, gtk::Justification::Right),
                "justify" | "justified" => (0.0, gtk::Justification::Fill),
                _ => (0.0, gtk::Justification::Left),
            };
            if let Some(label) = view.downcast_ref::<gtk::Label>() {
                label.set_xalign(x);
                label.set_justify(justify);
            } else if let Some(entry) = view.downcast_ref::<gtk::Entry>() {
                EditableExt::set_alignment(entry, x);
            }
        }
        (Kind::Text, "lines") => {
            if let Some(label) = view.downcast_ref::<gtk::Label>() {
                let lines = num(&value).max(0.0) as i32;
                label.set_wrap(lines != 1);
                label.set_lines(if lines == 0 { -1 } else { lines });
                label.set_ellipsize(if lines == 0 {
                    pango::EllipsizeMode::None
                } else {
                    pango::EllipsizeMode::End
                });
            }
            app.layouts.mark_dirty(handle);
            relayout = true;
        }
        (Kind::Text, "selectable") => {
            if let Some(label) = view.downcast_ref::<gtk::Label>() {
                label.set_selectable(truthy(&value));
            }
        }

        // Button
        (Kind::Button, "label" | "icon") => {
            if name == "label" {
                widget.button.0 = text(&value);
            } else {
                widget.button.1 = text(&value);
            }
            if let Some(button) = view.downcast_ref::<gtk::Button>() {
                rebuild_button(button, &widget.button.0, &widget.button.1);
            }
            app.layouts.mark_dirty(handle);
            relayout = true;
        }
        (Kind::Button, "variant") => {
            let variant = text(&value);
            for class in ["suggested-action", "destructive-action", "flat"] {
                view.remove_css_class(class);
            }
            match variant.as_str() {
                "primary" => view.add_css_class("suggested-action"),
                "destructive" | "danger" => view.add_css_class("destructive-action"),
                "plain" | "link" => view.add_css_class("flat"),
                _ => {}
            }
            let tint = match variant.as_str() {
                "link" => "link",
                _ => "",
            };
            set_color(widget, &view, tint);
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
            for class in ["kiln-size-small", "kiln-size-mini", "kiln-size-large"] {
                view.remove_css_class(class);
            }
            match text(&value).as_str() {
                "sm" | "small" => view.add_css_class("kiln-size-small"),
                "xs" | "mini" => view.add_css_class("kiln-size-mini"),
                "lg" | "large" => view.add_css_class("kiln-size-large"),
                _ => {}
            }
            app.layouts.mark_dirty(handle);
            relayout = true;
        }

        // Inputs
        (Kind::TextInput | Kind::SecureInput, "value") => {
            if let Some(entry) = view.downcast_ref::<gtk::Entry>() {
                let next = text(&value);
                if entry.text() != next {
                    entry.set_text(&next);
                }
            }
        }
        (Kind::TextInput | Kind::SecureInput, "placeholder") => {
            if let Some(entry) = view.downcast_ref::<gtk::Entry>() {
                entry.set_placeholder_text(Some(&text(&value)));
            }
        }
        (Kind::TextArea, "value") => {
            if let Some(text_view) = inner.and_then(|w| w.downcast::<gtk::TextView>().ok()) {
                let buffer = text_view.buffer();
                let (start, end) = buffer.bounds();
                let next = text(&value);
                if buffer.text(&start, &end, false) != next {
                    buffer.set_text(&next);
                }
            }
        }

        // Ranges
        (Kind::Slider, "value" | "min" | "max") => {
            if let Some(scale) = view.downcast_ref::<gtk::Scale>() {
                let adjustment = scale.adjustment();
                match name {
                    "value" => scale.set_value(num(&value)),
                    "min" => scale.set_range(num(&value), adjustment.upper().max(num(&value))),
                    _ => scale.set_range(adjustment.lower().min(num(&value)), num(&value)),
                }
            }
        }
        (Kind::Progress, "value" | "min" | "max") => {
            match name {
                "value" => widget.range.2 = num(&value),
                "min" => widget.range.0 = num(&value),
                _ => widget.range.1 = num(&value),
            }
            update_progress(widget);
        }
        (Kind::Progress, "indeterminate") => {
            if truthy(&value) {
                if widget.pulse.is_none()
                    && let Some(bar) = view.downcast_ref::<gtk::ProgressBar>()
                {
                    let bar = bar.downgrade();
                    widget.pulse = Some(glib::timeout_add_local(
                        Duration::from_millis(100),
                        move || match bar.upgrade() {
                            Some(bar) => {
                                bar.pulse();
                                glib::ControlFlow::Continue
                            }
                            None => glib::ControlFlow::Break,
                        },
                    ));
                }
            } else if let Some(pulse) = widget.pulse.take() {
                pulse.remove();
                update_progress(widget);
            }
        }

        // Choices
        (Kind::Toggle, "checked") => {
            if let Some(switch) = view.downcast_ref::<gtk::Switch>() {
                switch.set_active(truthy(&value));
            }
        }
        (Kind::Checkbox, "checked") => {
            if let Some(check) = view.downcast_ref::<gtk::CheckButton>() {
                check.set_active(truthy(&value));
            }
        }
        (Kind::Checkbox, "label") => {
            if let Some(check) = view.downcast_ref::<gtk::CheckButton>() {
                let label = text(&value);
                check.set_label((!label.is_empty()).then_some(label.as_str()));
            }
            app.layouts.mark_dirty(handle);
            relayout = true;
        }
        (Kind::Select, "options") => {
            if let Some(dropdown) = view.downcast_ref::<gtk::DropDown>() {
                widget.options = string_list(&value);
                let items: Vec<&str> = widget.options.iter().map(String::as_str).collect();
                dropdown.set_model(Some(&gtk::StringList::new(&items)));
            }
            app.layouts.mark_dirty(handle);
            relayout = true;
        }
        (Kind::Select, "selectedIndex") => {
            if let Some(dropdown) = view.downcast_ref::<gtk::DropDown>() {
                let index = num(&value);
                dropdown.set_selected(if index < 0.0 {
                    gtk::INVALID_LIST_POSITION
                } else {
                    index as u32
                });
            }
        }
        (Kind::Segmented, "options") => {
            if let Some(row) = view.downcast_ref::<gtk::Box>() {
                for old in widget.segments.drain(..) {
                    row.remove(&old);
                }
                let items = string_list(&value);
                for (i, item) in items.iter().enumerate() {
                    let segment = gtk::ToggleButton::with_label(item);
                    if let Some(first) = widget.segments.first() {
                        segment.set_group(Some(first));
                    }
                    let label = item.clone();
                    segment.connect_toggled(move |s| {
                        if s.is_active() {
                            push(handle, EventKind::Change, i as f64, 0.0, label.clone());
                        }
                    });
                    row.append(&segment);
                    widget.segments.push(segment);
                }
            }
            app.layouts.mark_dirty(handle);
            relayout = true;
        }
        (Kind::Segmented, "selectedIndex") => {
            let index = num(&value);
            if index >= 0.0
                && let Some(segment) = widget.segments.get(index as usize)
            {
                segment.set_active(true);
            } else {
                for segment in &widget.segments {
                    segment.set_active(false);
                }
            }
        }

        // Images
        (Kind::Image, "src" | "symbol" | "name") => {
            if let Some(image) = view.downcast_ref::<gtk::Image>() {
                let name = text(&value);
                if name.is_empty() {
                    image.clear();
                } else if is_path(&name) {
                    image.set_from_file(Some(expand_home(&name)));
                } else {
                    image.set_icon_name(Some(&icon_name(&name)));
                }
            }
            app.layouts.mark_dirty(handle);
            relayout = true;
        }
        (Kind::Image, "size") => {
            if let Some(image) = view.downcast_ref::<gtk::Image>() {
                image.set_pixel_size(num(&value).round() as i32);
            }
            app.layouts.mark_dirty(handle);
            relayout = true;
        }
        _ => {}
    }
    if relayout {
        invalidate(app, handle);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Tree

/// The KilnBox a parent's children go into: a scroll view's document, else the parent itself.
fn host_of(widget: &Widget) -> gtk::Widget {
    match (widget.kind, &widget.inner) {
        (Kind::Scroll, Some(doc)) => doc.clone(),
        _ => widget.view.clone(),
    }
}

pub fn insert(parent: u32, child: u32, before: u32) -> Result<(), String> {
    with_app(|app| {
        let parent_widget = app
            .widgets
            .get(&parent)
            .ok_or_else(|| format!("no widget {parent}"))?;
        if !parent_widget.kind.holds_children() {
            return Err(format!("{:?} cannot hold children", parent_widget.kind));
        }
        let parent_kind = parent_widget.kind;
        let host = host_of(parent_widget);
        let sibling = (before != 0)
            .then(|| app.widgets.get(&before).map(|w| w.view.clone()))
            .flatten()
            .filter(|s| s.parent().as_ref() == Some(&host));
        let child_widget = app
            .widgets
            .get_mut(&child)
            .ok_or_else(|| format!("no widget {child}"))?;
        if child_widget.kind == Kind::Window {
            return Err("a window cannot be placed inside another widget".into());
        }
        child_widget.parent = parent;
        let child_view = child_widget.view.clone();
        if let Some(old) = child_view.parent()
            && old != host
        {
            child_view.unparent();
        }
        if sibling.as_ref() != Some(&child_view) {
            child_view.insert_before(&host, sibling.as_ref());
        }
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
}

pub fn remove(parent: u32, child: u32) -> Result<(), String> {
    with_app(|app| {
        let parent_kind = app.widgets.get(&parent).map(|w| w.kind);
        let layout_parent = if parent_kind == Some(Kind::Scroll) {
            parent | SCROLL_DOCUMENT_BIT
        } else {
            parent
        };
        app.layouts.remove(layout_parent, child);
        if let Some(widget) = app.widgets.get_mut(&child) {
            widget.parent = 0;
            if widget.view.parent().is_some() {
                widget.view.unparent();
            }
        }
        invalidate(app, parent);
        Ok(())
    })
}

pub fn destroy(handle: u32) -> Result<(), String> {
    let window = with_app(|app| {
        let Some(mut widget) = app.widgets.remove(&handle) else {
            return Ok(None);
        };
        if widget.parent != 0 {
            invalidate(app, widget.parent);
        }
        app.layouts.drop_node(handle);
        if widget.kind == Kind::Scroll {
            app.layouts.drop_node(handle | SCROLL_DOCUMENT_BIT);
        }
        if let Some(pulse) = widget.pulse.take() {
            pulse.remove();
        }
        REGISTRY.with(|r| {
            let mut r = r.borrow_mut();
            r.remove(&key(&widget.view));
            if let Some(inner) = &widget.inner {
                r.remove(&key(inner));
            }
        });
        PLACED.with(|p| p.borrow_mut().remove(&key(&widget.view)));
        if let Some(parts) = widget.window.take() {
            // Destroying does not emit close-request; do it after the state is released.
            return Ok(Some(parts.window));
        }
        if widget.view.parent().is_some() {
            widget.view.unparent();
        }
        Ok(None)
    })?;
    if let Some(window) = window {
        window.destroy();
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Commands

/// What GTK holds for `handle`'s assistive technology, as JSON: `label`, `hint`, `element`
/// (whether it is announced at all) and `focused`. The role is kiln's, from the widget kind.
/// GTK has no getters for accessible properties, so the label and hint are the ones this backend
/// handed it.
pub fn describe(handle: u32) -> Result<String, String> {
    let (kind, view, inner, label, hint, pressable) = with_app(|app| {
        let widget = app
            .widgets
            .get(&handle)
            .ok_or_else(|| format!("no widget {handle}"))?;
        Ok((
            widget.kind,
            widget.view.clone(),
            widget.inner.clone(),
            widget.label.clone(),
            widget.hint.clone(),
            widget.pressable.is_some(),
        ))
    })?;
    let target = match (kind, inner) {
        (Kind::TextArea, Some(inner)) => inner,
        _ => view.clone(),
    };
    let element = match kind {
        Kind::Window | Kind::Container | Kind::Spacer => pressable,
        Kind::Image => !label.is_empty(),
        Kind::Divider | Kind::Scroll => false,
        _ => target.accessible_role() != gtk::AccessibleRole::Presentation,
    };
    let focused = view
        .root()
        .and_then(|root| gtk::prelude::RootExt::focus(&root))
        .is_some_and(|focus| focus == target || focus.is_ancestor(&target));
    Ok(serde_json::json!({
        "label": label,
        "hint": hint,
        "element": element,
        "focused": focused,
    })
    .to_string())
}

/// `show`, `hide`, `close`, `minimize`, `maximize`, `unmaximize`, `fullscreen` (arg "true" or
/// "false"), `center`, `focus` and `snapshot` (arg: a PNG path) for windows; `click` and `focus`
/// for controls.
pub fn call(handle: u32, command: &str, arg: &str) -> Result<i32, String> {
    let (kind, view, inner, window, pressable) = with_app(|app| {
        let widget = app
            .widgets
            .get(&handle)
            .ok_or_else(|| format!("no widget {handle}"))?;
        Ok((
            widget.kind,
            widget.view.clone(),
            widget.inner.clone(),
            widget.window.as_ref().map(|p| p.window.clone()),
            widget.pressable.is_some(),
        ))
    })?;
    let Some(window) = window else {
        return match command {
            // A real click: the control's own signal runs, and its handler queues the event.
            "click" => {
                if let Some(button) = view.downcast_ref::<gtk::Button>() {
                    button.emit_clicked();
                } else if let Some(switch) = view.downcast_ref::<gtk::Switch>() {
                    switch.set_active(!switch.is_active());
                } else if pressable {
                    push(handle, EventKind::Press, 0.0, 0.0, String::new());
                } else {
                    view.activate();
                }
                Ok(0)
            }
            "focus" => {
                let target = match (kind, inner) {
                    (Kind::TextArea, Some(inner)) => inner,
                    _ => view,
                };
                target.grab_focus();
                Ok(0)
            }
            _ => Err(format!("{kind:?} has no command `{command}`")),
        };
    };
    match command {
        "show" => {
            window.present();
            queue_containers(&view);
        }
        "hide" => window.set_visible(false),
        "close" => window.close(),
        "minimize" => window.minimize(),
        "maximize" => window.maximize(),
        "unmaximize" => window.unmaximize(),
        "fullscreen" => {
            if arg == "true" {
                window.fullscreen()
            } else {
                window.unfullscreen()
            }
        }
        // GTK 4 leaves placement to the window manager; there is nothing to ask for.
        "center" => {}
        "focus" => window.present(),
        "snapshot" => snapshot(&window, arg)?,
        other => return Err(format!("windows have no command `{other}`")),
    }
    Ok(0)
}

/// Renders the window's content (menu bar and root, over the window background) to a PNG:
/// the widget's render nodes, through the window's renderer, into a texture.
fn snapshot(window: &gtk::Window, path: &str) -> Result<(), String> {
    let context = glib::MainContext::default();
    for _ in 0..100 {
        if !context.iteration(false) {
            break;
        }
    }
    let content = window
        .child()
        .ok_or_else(|| "the window has no content".to_string())?;
    let (width, height) = (content.width(), content.height());
    if !content.is_mapped() || width <= 0 || height <= 0 {
        return Err("show the window before taking a snapshot".into());
    }
    let bounds = graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    let snapshot = gtk::Snapshot::new();
    snapshot.append_color(&rgba("background"), &bounds);
    let paintable = gtk::WidgetPaintable::new(Some(&content));
    paintable.snapshot(&snapshot, width as f64, height as f64);
    let node = snapshot
        .to_node()
        .ok_or_else(|| "the window drew nothing".to_string())?;
    let renderer = window
        .renderer()
        .ok_or_else(|| "the window has no renderer".to_string())?;
    let texture = renderer.render_texture(&node, Some(&bounds));
    texture
        .save_to_png(path)
        .map_err(|e| format!("could not write {path}: {e}"))
}

// ---------------------------------------------------------------------------------------------
// Events

/// Runs the GLib main context until an event for the program arrives or `timeout_ms` passes,
/// then dispatches everything else already pending. A negative timeout blocks until an event
/// (or [`wake`]); zero only dispatches what is pending. Returns whether the kiln event queue
/// holds anything for the program.
pub fn wait(timeout_ms: f64) -> Result<bool, String> {
    if APP.with(|cell| cell.borrow().is_none()) {
        return Err("kiln:gui is not initialised on this thread".into());
    }
    flush_dirty_windows();
    let context = glib::MainContext::default();
    if events::is_empty() && timeout_ms != 0.0 && !WOKEN.swap(false, Ordering::AcqRel) {
        let expired = Rc::new(Cell::new(false));
        let timer = (timeout_ms > 0.0).then(|| {
            let expired = expired.clone();
            glib::timeout_add_local_once(Duration::from_secs_f64(timeout_ms / 1000.0), move || {
                expired.set(true)
            })
        });
        while events::is_empty() && !expired.get() {
            context.iteration(true);
            if WOKEN.swap(false, Ordering::AcqRel) {
                break;
            }
        }
        if let Some(timer) = timer
            && !expired.get()
        {
            timer.remove();
        }
    }
    for _ in 0..1000 {
        if !context.iteration(false) {
            break;
        }
    }
    Ok(!events::is_empty())
}

/// Makes a [`wait`] in progress on the main thread return. Safe to call from any thread.
pub fn wake() {
    WOKEN.store(true, Ordering::Release);
    glib::MainContext::default().wakeup();
}

// ---------------------------------------------------------------------------------------------
// Menus

/// `"CmdOrCtrl+Shift+S"` -> `"<Control><Shift>s"`.
fn accelerator(spec: &str) -> String {
    let mut modifiers = String::new();
    let mut key = String::new();
    for part in spec.split('+') {
        let lower = part.trim().to_ascii_lowercase();
        match lower.as_str() {
            "cmd" | "command" | "cmdorctrl" | "commandorcontrol" | "ctrl" | "control" => {
                modifiers.push_str("<Control>")
            }
            "super" | "meta" => modifiers.push_str("<Super>"),
            "alt" | "option" | "opt" => modifiers.push_str("<Alt>"),
            "shift" => modifiers.push_str("<Shift>"),
            "enter" | "return" => key = "Return".into(),
            "tab" => key = "Tab".into(),
            "space" => key = "space".into(),
            "backspace" => key = "BackSpace".into(),
            "delete" => key = "Delete".into(),
            "escape" | "esc" => key = "Escape".into(),
            "up" | "down" | "left" | "right" => {
                key = format!("{}{}", lower[..1].to_ascii_uppercase(), &lower[1..])
            }
            other if other.len() > 1 && other.starts_with('f') => key = other.to_ascii_uppercase(),
            other => key = other.to_string(),
        }
    }
    format!("{modifiers}{key}")
}

/// Builds `items` into `menu`, registering an action and (when it has one) a shortcut for each.
fn build_items(
    menu: &gio::Menu,
    items: Option<&Value>,
    actions: &gio::SimpleActionGroup,
    shortcuts: &gtk::ShortcutController,
    next: &mut usize,
) {
    let Some(Value::Array(items)) = items else {
        return;
    };
    let mut section = gio::Menu::new();
    for item in items {
        if item.get("separator").is_some_and(truthy) || item.as_str() == Some("separator") {
            menu.append_section(None, &section);
            section = gio::Menu::new();
            continue;
        }
        let label = item.get("label").map(text).unwrap_or_default();
        if let Some(children) = item.get("items") {
            let sub = gio::Menu::new();
            build_items(&sub, Some(children), actions, shortcuts, next);
            section.append_submenu(Some(&label), &sub);
            continue;
        }
        let name = format!("m{next}");
        *next += 1;
        let id = item.get("id").map(text).unwrap_or_else(|| label.clone());
        let action = gio::SimpleAction::new(&name, None);
        action.connect_activate(move |_, _| push(0, EventKind::Menu, 0.0, 0.0, id.clone()));
        if item.get("enabled").is_some_and(|e| !truthy(e)) {
            action.set_enabled(false);
        }
        actions.add_action(&action);
        let detailed = format!("kiln.{name}");
        section.append(Some(&label), Some(&detailed));
        if let Some(spec) = item.get("accelerator").map(text)
            && let Some(trigger) = gtk::ShortcutTrigger::parse_string(&accelerator(&spec))
        {
            shortcuts.add_shortcut(gtk::Shortcut::new(
                Some(trigger),
                Some(gtk::NamedAction::new(&detailed)),
            ));
        }
    }
    menu.append_section(None, &section);
}

/// Gives `parts` the program's menus: a menu bar above the root (shown only when there are
/// menus), their actions, and their shortcuts, plus Ctrl+Q for quit.
fn install_menu(parts: &mut WindowParts, menus: &[Value]) {
    let actions = gio::SimpleActionGroup::new();
    let quit = gio::SimpleAction::new("quit", None);
    quit.connect_activate(|_, _| push(0, EventKind::Quit, 0.0, 0.0, String::new()));
    actions.add_action(&quit);
    parts.window.remove_controller(&parts.shortcuts);
    let shortcuts = gtk::ShortcutController::new();
    if let Some(trigger) = gtk::ShortcutTrigger::parse_string("<Control>q") {
        shortcuts.add_shortcut(gtk::Shortcut::new(
            Some(trigger),
            Some(gtk::NamedAction::new("kiln.quit")),
        ));
    }
    let bar = gio::Menu::new();
    let mut next = 0;
    for menu in menus {
        let title = menu.get("label").map(text).unwrap_or_default();
        let sub = gio::Menu::new();
        build_items(&sub, menu.get("items"), &actions, &shortcuts, &mut next);
        bar.append_submenu(Some(&title), &sub);
    }
    parts.window.insert_action_group("kiln", Some(&actions));
    parts.window.add_controller(shortcuts.clone());
    parts.shortcuts = shortcuts;
    parts.menubar.set_menu_model(Some(&bar));
    parts.menubar.set_visible(!menus.is_empty());
}

/// Replaces the program's menus with `spec`, a JSON array of
/// `{ label, items: [{ id, label, accelerator, enabled, checked, items } | { separator: true }] }`.
/// GTK has no application-wide menu bar outside a GtkApplication, so every window shows them.
pub fn set_menu(spec: &str) -> Result<(), String> {
    let menus: Value = serde_json::from_str(spec).map_err(|e| format!("menu: {e}"))?;
    let menus = menus.as_array().cloned().unwrap_or_default();
    with_app(|app| {
        app.menus = menus;
        let menus = app.menus.clone();
        for widget in app.widgets.values_mut() {
            if let Some(parts) = &mut widget.window {
                install_menu(parts, &menus);
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accelerators_use_gtk_syntax() {
        assert_eq!(accelerator("CmdOrCtrl+Shift+K"), "<Control><Shift>k");
        assert_eq!(accelerator("Alt+F4"), "<Alt>F4");
        assert_eq!(accelerator("Ctrl+Enter"), "<Control>Return");
    }

    #[test]
    fn colors_parse_and_resolve() {
        let c = parse_hex("ff8000").unwrap();
        assert_eq!((c.red(), c.alpha()), (1.0, 1.0));
        assert!(parse_hex("12").is_none());
        assert_eq!(rgba("clear").alpha(), 0.0);
        assert!(rgba("secondary").alpha() < rgba("label").alpha());
    }

    #[test]
    fn kinds_parse_like_appkit() {
        assert_eq!(Kind::parse("hstack"), Some(Kind::Container));
        assert_eq!(Kind::parse("card"), Some(Kind::Container));
        assert!(Kind::parse("webview").is_none());
        assert!(Kind::Scroll.holds_children());
        assert!(!Kind::Divider.is_measured());
    }
}
