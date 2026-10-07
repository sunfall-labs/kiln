# ADR 0074: kiln:gui — native widgets, one layout model, and the host run loop

## Status

Accepted, 2026-10-02. Supersedes the 2026-09-29 proposal of the same number, which described a
design (Solid 2.0 via `solid-js/universal`, a lock-free turn mailbox, zero-CPU idle, a statically
linked provider) that the implementation never had. This version describes what exists and names
what does not yet.

Builds on ADR 0062 (demand-driven provider capabilities), ADR 0068 (kiln extension modules),
ADR 0070 (realm turns) and ADR 0071 (process-wide caches and realms side by side).

## Context

kiln compiles TypeScript to native binaries. A desktop program should be able to show real
platform widgets — not a bundled browser — with a declarative, reactive programming model, and
without the GUI distorting the compiler, the C backend, or the parity between the interpreter
and the native backend.

## Decision

### 1. One engine-owned module, in the `kiln:` scheme

`kiln:gui` is a kiln extension module (ADR 0068). Its JSX runtime is `kiln:gui/jsx-runtime`
(and `kiln:gui/jsx-dev-runtime`), so a program sets `"jsxImportSource": "kiln:gui"` or
`/** @jsxImportSource kiln:gui */`.

Every engine-owned module lives under `kiln:` — `kiln:<library>` and `kiln:<library>/<subpath>` —
exactly as Node's live under `node:`. A bare name (`kiln/jsx-runtime`) would sit in npm's
namespace, shadow a real package called `kiln`, and need a special case in every resolver; the
first version did use one, and `kiln run` could not resolve it.

The module is TypeScript (`crates/semantics/kiln-builtins/src/gui.ts`), compiled into both
engines like any builtin. It contains, bottom to top:

1. **Reactivity.** Signals, memos and effects with owners, in the push-pull style: a write marks
   observers stale; an effect pulls from its sources when it runs. It is glitch-free (an effect
   never sees two sources from different moments), a memo whose value did not change stops
   propagation, and a write outside `batch` has run its effects before it returns. Owners give
   cleanup, context and disposal. Memos may return promises (`isPending`). The API follows
   SolidJS's names because they are the best-known vocabulary for this model; it is kiln's own
   implementation and not compatible with `solid-js` itself.
2. **Control flow.** `Show`, `For` (keyed by identity), `Index`, `Switch`/`Match`, `Dynamic`,
   `createContext`. Each renders where it decides, so its widgets belong to that decision and
   survive anything else re-running.
3. **Hosts.** The `Host` contract is ten operations: create, set, insert (before a sibling),
   remove, destroy, call (window commands), wait, next event, plus OS services. Two hosts exist:
   the native library and an in-memory `HeadlessHost`.
4. **Widgets.** `Node` and its subclasses. A node exists in JavaScript from construction; its
   native widget exists only while it is attached to a shown window. Removing a node keeps it;
   disposing it — directly, or because the reactive owner that created it was disposed — destroys
   its whole subtree.
5. **JSX.** Without a JSX compiler of its own, kiln evaluates JSX eagerly, children first. So
   `jsx()` returns a description (`JsxElement`), not a widget: components then run parent-first,
   which is what context, ownership and conditional rendering require. A dynamic expression is
   written as a function (`{() => count()}`, `when={visible}`); `insert` turns each function
   into a region that re-renders only its own part of the parent's children.

### 2. Layout is decided in TypeScript and computed by Taffy

Every container — window, stacks, grid, card, scroll document — is a flexbox or grid container.
The TypeScript side owns layout *semantics*: each widget class supplies defaults (an `HStack` is
a centred row with a gap; a `Button` in a column does not stretch; a `TextInput` in a row
grows), then the `style` object and individual props (`grow`, `padding`, `align`, `columns`…)
refine them, and the host receives one `layout` object. The native library parses it into a
Taffy style; the platform only measures leaves (a label's wrapped height, a button's natural
size). A window's root view re-runs the layout from the toolkit's own layout pass (AppKit's
`layout`, GTK's `size_allocate`), so live resizing re-flows without a round trip through
JavaScript.

This replaced a mix of `NSStackView` autolayout for stacks and Taffy for boxes, where widths
were added as constraints on every write and accumulated.

### 3. The native library is a separate dynamic library with a versioned C ABI

`crates/host/kiln-host-gui` builds `libkiln_host_gui`, which `gui.ts` binds with the FFI the
native backend already provides (`process.__kilnNativeModules.ffi`). Its ABI
(`crates/host/kiln-host-gui/src/ffi.rs`) is deliberately uniform: strings in are NUL-terminated,
text out goes through one result buffer, `-1` is "nothing", `-2` is an error whose message the
caller reads and throws, handles are `u32` allocated by the library, and nothing unwinds across
it. `kiln_gui_abi_version` lets the binding refuse a mismatched library by name.

The library is not found at run time; it is linked. `kiln-host-gui` is a feature (`gui`) of the
one provider archive, `kiln-native-provider-ffi`, carried by the `gui` and `full-gui` variants
(`full` deliberately stops short of it, so a headless program never links AppKit), and the native
`node:ffi` engine answers `dlopen("kiln:host-gui")` from the table the archive carries
(`kiln_host_gui::ffi::exports`). That table is also what keeps the bridge alive through
whole-archive LTO. The platform libraries a `gui` archive adds to the link line — AppKit's
frameworks on macOS, GTK 4 and libwayland-client on Linux — are listed in the contract
(`gui::link_arguments`) and proven at packaging time by linking a probe that references every
export. On Linux they are shared libraries: linking a GUI program needs the GTK 4 development
files, running it needs GTK 4, and a fully static link (`--static`) cannot carry them. `KILN_GUI_LIB` still points a program at a shared library instead, as a developer
override for iterating on the Rust side.

It has two widget backends behind the same contract (the functions of the stub `backend` module
in `ffi.rs`):

* **AppKit** (`src/appkit.rs`) on macOS.
* **GTK 4** (`src/gtk.rs`) on Linux, through gtk4-rs, behind the crate's default `gtk` feature.
  It needs GTK 4.10 or newer, and its development files to build (`libgtk-4-dev`); with
  `--no-default-features` the library builds without it and reports that there is no backend.
  Every container — window root, stack, card, scroll document — is one custom widget
  (`KilnBox`) whose `size_allocate` places its children at the frames Taffy computed, and
  leaves are measured with `gtk_widget_measure`, so GTK's own layout containers (`GtkBox`,
  `GtkGrid`) are never used. Semantic colors resolve against the GNOME palette and reach text
  and symbols as generated CSS classes; SF Symbol names map to freedesktop icon names
  (`trash` → `user-trash-symbolic`) and otherwise pass through. GTK has no application-wide
  menu bar outside a `GtkApplication`, so the program's menus are a menu bar at the top of each
  window, with their accelerators as window shortcuts (`CmdOrCtrl` is Control). `wait` iterates
  the default GLib main context; `wake` (any thread) interrupts it.

On other platforms `kiln_gui_init` reports that there is none.

### 4. Main-thread ownership without locks

AppKit and GTK are main-thread toolkits. Each backend keeps every piece of state in
`thread_local!` storage and refuses to initialise anywhere but the main thread, so a realm on
another thread gets an error naming the cause instead of a crash or a mutex. Handles are
allocated by the library, never derived from JavaScript ids, so realms side by side (ADR 0071)
cannot collide.

### 5. Events: queued natively, dispatched in batches, never re-entrant

Toolkit callbacks never call JavaScript. They append to a queue (`src/events.rs`) — including
callbacks AppKit fires synchronously inside a property write, such as a resize caused by
`setContentSize:`. The program drains the queue between its own turns and dispatches each
drain as one `batch`. This keeps run-to-completion intact by construction.

A widget that reports user input updates its props first, emits the event, and then restores any
*bound* prop the handler did not follow: a `value={signal}` input is controlled, exactly like a
controlled input elsewhere.

### 6. The event loop: the realm parks inside the OS run loop

The OS delivers events to the main thread's run loop; kiln's own work (timers, I/O) runs on its
realm loop, a Tokio current-thread runtime whose idle wait is its I/O driver. While a window is
open, the realm *parks inside the OS run loop* instead (`kiln_runtime::ParkHook`, installed by
the bridge through `kiln_gui_park`):

* The realm's single park point (`drive_async`) calls the hook's `wait` with the exact time to
  its next timer. With no I/O in flight that is one wait, ended by an OS event, the deadline, or
  a completion from any thread — the completion signal is polled with a waker that calls the
  hook's thread-safe `wake` (`postEvent:` on AppKit, `g_main_context_wakeup` on GTK). An idle app uses no CPU and timers fire on
  time (measured: within 2 ms; the smoke test holds it under 15 ms).
* While I/O needs the driver turned (sockets, host tasks), the wait is sliced at 4 ms and the
  driver gets a zero-timeout turn between slices.
* OS events are announced on a runtime endpoint; `kiln:gui` listens with a message port bound
  to it (the engine's own port internals), which also keeps the program alive while it listens.

`eventLoopMode()` reports `"parked"`, `"polling"` or `"stopped"`. A library that cannot park —
the development dylib loaded with `KILN_GUI_LIB`, which is not linked with the runtime — falls
back to bounded polling: dispatch, wait up to 8 ms (50 ms idle), yield with a 1 ms timer. Input
is still immediate there; timers and I/O can be a slice late. The interpreter is headless and
needs neither.

### 7. Headless is a host, not a fallback that pretends

The interpreter has no FFI, so there it — and anywhere the native library cannot start — uses
`HeadlessHost`, which builds the same tree in memory. When a window is shown on a host chosen
by fallback, `kiln:gui` prints once *why* (`this engine has no FFI…`, `libkiln_host_gui.dylib not
found (looked in …)`) and how to silence it (`KILN_GUI=headless`). Tests call `useHeadless()`;
`fire(node, event, value)` delivers a user action through the same path a native event takes,
and `processEvents()` drains host events.

The headless host is what makes `conformance/native/gui-*.ts` meaningful: the same program runs
in the interpreter and the native backend and must print the same thing.

### 8. Accessibility and keyboard focus

Every widget takes `accessibilityLabel` and `accessibilityHint`. A container with `onPress` is
announced as a button and VoiceOver can press it (`accessibilityPerformPress` delivers the same
event a click does); an unlabelled symbol image is decorative and not announced. Tab and
Shift-Tab move through controls in layout order, and `autofocus` gives a text field focus when
its window is shown. `node.describe()` reports what assistive technology sees: the role is kiln's
(from the widget kind, the same word on every host), and the host reports the label, hint,
whether the widget is announced, and whether it has focus.

### 9. Capability planning

`kiln:gui` and `kiln:gui/jsx-runtime` retain the `gui` provider capability (ADR 0062); a program
that does not import them plans none of it. Because the widget library is loaded with FFI at
run time, the native build does not yet link it: shipping a GUI binary means shipping
`libkiln_host_gui` beside it.

## Verification

* `conformance/native/gui-reactivity.ts`, `conformance/native/gui-widgets.ts` — the core and the
  widget layer on the headless host, in both engines (`scripts/native.sh`).
* `scripts/gui-smoke.sh` — builds `conformance/gui/native-smoke.tsx` and runs it against the real
  window system: toolkit clicks reach handlers, timers run beside the UI loop, resize and close
  events arrive. It needs a display, so it is not in the gate. On macOS it needs a logged-in
  session. On Linux it needs `DISPLAY` or `WAYLAND_DISPLAY` and the GTK 4 development files;
  headless, run it under Xvfb: `xvfb-run -a scripts/gui-smoke.sh`. Without a display it skips.
  A minimal image also wants `librsvg2-common`: without the SVG pixbuf loader, symbolic icons
  at sizes other than 16 px draw as "missing image".
  `KILN_GUI_SNAPSHOT=out.png ./program` writes the first window of a `runApp` program to a PNG
  half a second after it is shown, on either backend.
* `cargo test -p kiln-host-gui` — style parsing, layout, the event queue.
* `scripts/gen-kiln-types.sh --check` — the published declarations (`@kiln/types`) match the implementation and
  the examples type-check against them.

## Not done yet

* **A Windows backend** behind the same ABI (Win32/WinUI), with the same Taffy layout.
* **On GTK:** `controlSize` is approximated with CSS; `material` is ignored; window `center` is
  a no-op (GTK 4 leaves placement to the window manager); checked menu items show no check
  mark; a `primary` button is styled but is not the window's Return-key default; text areas
  take `monospace` but not the type ramp; `describe` reports the label and hint the backend
  handed GTK, which has no getters for them. A pressable container's button role can only be
  set before assistive technology realises the widget.
* **Native binaries on Linux arm64:** the native backend's value boxing keeps 47 address bits,
  and arm64 Linux places the `brk` heap above 2^47. On Ubuntu 24.04 arm64 every native
  program, `examples/hello.ts` included, segfaults at a pointer with bit 47 cleared, so the GTK
  backend has been verified on x86_64 only (Ubuntu 24.04, GTK 4.14, Xvfb).
* Drag and drop, images from memory, tables, custom focus order beyond layout order.

## Rejected alternatives

* **Embedding a web view as the UI** (Electron, Tauri): forfeits native controls; ADR 0075
  covers web views as one widget among others.
* **A virtual DOM** (`react-reconciler`): re-runs components and diffs trees on every update.
* **Depending on `solid-js`**: its universal renderer expects a compiler that wraps expressions
  in getters, which kiln's JSX does not do; a small, owned core that fits the eager JSX is
  simpler than adapting it.
* **Ids chosen by JavaScript**: collide between realms and leak the binding's bookkeeping into
  the toolkit.
* **Platform layout per toolkit** (stack views, GtkBox): layout would differ by platform and by
  widget; one Taffy model means one set of semantics, decided in one place.
