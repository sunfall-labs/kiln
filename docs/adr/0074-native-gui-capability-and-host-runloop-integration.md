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
size). A window's root view re-runs the layout from AppKit's `layout` pass, so live resizing
re-flows without a round trip through JavaScript.

This replaced a mix of `NSStackView` autolayout for stacks and Taffy for boxes, where widths
were added as constraints on every write and accumulated.

### 3. The native library is a separate dynamic library with a versioned C ABI

`crates/host/kiln-host-gui` builds `libkiln_host_gui`, which `gui.ts` binds with the FFI the
native backend already provides (`process.__kilnNativeModules.ffi`). Its ABI
(`crates/host/kiln-host-gui/src/ffi.rs`) is deliberately uniform: strings in are NUL-terminated,
text out goes through one result buffer, `-1` is "nothing", `-2` is an error whose message the
caller reads and throws, handles are `u32` allocated by the library, and nothing unwinds across
it. `kiln_gui_abi_version` lets the binding refuse a mismatched library by name.

The library is found through `KILN_GUI_LIB`, next to the executable (`./`, `../lib`,
`../Frameworks`), or in the working tree's `target/` for development.

Today it has one widget backend, AppKit (`src/appkit.rs`). On other platforms `kiln_gui_init`
reports that there is none.

### 4. Main-thread ownership without locks

AppKit is a main-thread toolkit. The backend keeps every piece of state in `thread_local!`
storage and refuses to initialise anywhere but the main thread, so a realm on another thread
gets an error naming the cause instead of a crash or a mutex. Handles are allocated by the
library, never derived from JavaScript ids, so realms side by side (ADR 0071) cannot collide.

### 5. Events: queued natively, dispatched in batches, never re-entrant

Toolkit callbacks never call JavaScript. They append to a queue (`src/events.rs`) — including
callbacks AppKit fires synchronously inside a property write, such as a resize caused by
`setContentSize:`. The program drains the queue between its own turns and dispatches each
drain as one `batch`. This keeps run-to-completion intact by construction.

A widget that reports user input updates its props first, emits the event, and then restores any
*bound* prop the handler did not follow: a `value={signal}` input is controlled, exactly like a
controlled input elsewhere.

### 6. The event loop: bounded waits, honestly

The OS delivers events to the main thread's run loop; kiln's own work (timers, I/O) runs on its
realm loop, a Tokio current-thread runtime whose idle wait is its I/O driver (kqueue on macOS).
Neither wait can see the other's sources today. So, while a window is open, each tick:

1. dispatches queued events;
2. waits in the OS run loop for up to 8 ms when something happened in the last 500 ms, or 50 ms
   when idle, returning at once when an OS event arrives;
3. dispatches again and schedules the next tick with a 1 ms timer, so the realm loop parks in
   its own driver and runs its I/O and timers.

Input is handled as soon as it arrives. Timers and I/O completions are at most one slice late.
Measured idle cost of the to-do example: about 20 ms of CPU in 5 s (0.2–0.4% of a core).

This is not zero-CPU idle. That needs a runtime hook that lets the realm's park wait on the OS
run loop and its own driver together; it is the follow-up below, not a claim.

### 7. Headless is a host, not a fallback that pretends

The interpreter has no FFI, so there it — and anywhere the native library cannot start — uses
`HeadlessHost`, which builds the same tree in memory. When a window is shown on a host chosen
by fallback, `kiln:gui` prints once *why* (`this engine has no FFI…`, `libkiln_host_gui.dylib not
found (looked in …)`) and how to silence it (`KILN_GUI=headless`). Tests call `useHeadless()`;
`fire(node, event, value)` delivers a user action through the same path a native event takes,
and `processEvents()` drains host events.

The headless host is what makes `conformance/native/gui-*.ts` meaningful: the same program runs
in the interpreter and the native backend and must print the same thing.

### 8. Capability planning

`kiln:gui` and `kiln:gui/jsx-runtime` retain the `gui` provider capability (ADR 0062); a program
that does not import them plans none of it. Because the widget library is loaded with FFI at
run time, the native build does not yet link it: shipping a GUI binary means shipping
`libkiln_host_gui` beside it.

## Verification

* `conformance/native/gui-reactivity.ts`, `conformance/native/gui-widgets.ts` — the core and the
  widget layer on the headless host, in both engines (`scripts/native.sh`).
* `scripts/gui-smoke.sh` — builds `conformance/gui/native-smoke.tsx` and runs it against the real
  window system: AppKit clicks reach handlers, timers run beside the UI loop, resize and close
  events arrive. macOS only, so not in the gate.
* `cargo test -p kiln-host-gui` — style parsing, layout, the event queue.
* `scripts/gen-gui-types.sh --check` — the published declarations match the implementation and
  the examples type-check against them.

## Not done yet

* **Zero-CPU idle and exact timers:** a realm-park hook that waits on the OS run loop and the
  realm driver together, and turns OS events into realm turns.
* **Linux and Windows backends** behind the same ABI (GTK4, Win32/WinUI), with the same Taffy
  layout. The earlier TypeScript GTK driver drove `libgtk-4` directly, had its own layout
  semantics and was removed rather than kept as a second, untested model.
* **Linking the library into the binary** as a planned provider staticlib, so a GUI program is
  one file.
* Accessibility labels, focus order, drag and drop, images from memory, tables.

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
