# ADR 0074: Native GUI capability, host runloop integration, and platform widget seams

## Status

Proposed, 2026-09-29.

Follows ADR 0062 for demand-driven provider capabilities, ADR 0068 for kiln extension modules,
ADR 0070 for realm turns and schedulable quanta, ADR 0071 for shape caches, and ADR 0005 for the
HostOps ABI boundary.

## Context

Compiling TypeScript and JavaScript into standalone native client applications without bundling
Chromium or Node.js runtimes (the Electron/Tauri tax) is an active area of compiler development.
The Perry compiler (`perryts.com`) demonstrated that developers want to author native desktop and
mobile applications using TypeScript and declarative component paradigms (mapping directly to
native widgets such as SwiftUI, GTK4, and WinUI), achieving sub-10ms startup times and 15–20 MB
memory footprints.

However, Perry achieves this by coupling its compiler frontend (SWC) and backend (LLVM/Cranelift)
directly to platform UI frameworks and a custom NaN-boxed value representation. This couples the
compiler pipeline to operating system UI changes and forces every dynamic JavaScript edge case to be
solved directly in LLVM IR without an interpreter or generic fallback.

Kiln is uniquely positioned to provide a lightweight, cross-platform native GUI target with
significantly higher specification conformance (~95% Test262) and lower maintenance risk:

1. **`kiln:*` virtual built-in namespace (ADR 0068):** Kiln already isolates platform extensions
   (`kiln:actors`, `kiln:shared`, `kiln:process`) from the standard ECMAScript and Node namespaces.
2. **Demand-driven provider capabilities (ADR 0062):** `kiln-liveness` computes retained capability
   sets, linking only the necessary provider staticlibs. Programs that do not import GUI capabilities
   pay zero binary bytes and zero compile-time overhead.
3. **C99 backend with in-unit specialization (ADR 0016):** Emitted C99 compiles via standard host
   compilers (`clang`/`gcc`) without linking LLVM into the compiler, boots in ~2.6 ms, and retains
   guarded in-unit fallbacks for megamorphic or dynamic property accesses.
4. **Pure AOT compilation:** The native backend emits static native code, satisfying iOS App Store
   and game console rules that strictly prohibit runtime JIT compilation.

The architectural challenge is integrating native UI into Kiln without distorting the compiler core,
violating differential parity, or polluting headless and server builds.

## Decision

### 1. The `kiln:gui` Extension Module

The engine defines the virtual built-in module `kiln:gui`:

```ts
import { Window, VStack, HStack, Button, Text, TextInput } from "kiln:gui";

const win = new Window({ title: "Counter", width: 320, height: 240 });
let count = 0;

const label = new Text(`Count: ${count}`);
const btn = new Button({
  label: "Increment",
  onPress: () => {
    count += 1;
    label.text = `Count: ${count}`;
  },
});

win.content = new VStack({ spacing: 12, children: [label, btn] });
win.show();
```

* `kiln:gui` is an extension module subject to ADR 0068: it is guarded by `KILN_EXTENSIONS` and
  absent when `--no-kiln-extensions` is specified.
* High-level declarative frameworks (such as a native React reconciler package,
  `@kiln/react-native-gui`) sit entirely in userland or secondary packages, emitting imperative
  mutations to the underlying `kiln:gui` element tree.
* Component definitions, layout property names (`padding`, `spacing`, `alignment`), and event
  signatures adopt standardized, cross-platform TypeScript contracts.

### 2. Demand-Driven `gui` Capability Planning

Per ADR 0062, provider requirements are derived strictly from the retained graph by `kiln-liveness`:

* When `kiln:gui` is live, `kiln-liveness` emits the `gui` provider capability.
* The native build links the `gui` provider variant from `kiln-native-provider-gui` (a staticlib
  providing the native platform widget glue).
* If a program does not import `kiln:gui`, the capability is not planned, and no GUI staticlib or OS
  framework (Cocoa, GTK, WinUI) is linked. Headless CLI and server binaries remain completely
  untouched.

### 3. Separation of Concerns: What to Port vs. What to Reject

#### Ported from Perry (`perryts.com`):
1. **TypeScript Widget Contracts & Schemas:** Interface declarations (`.d.ts`), prop types, and
   event listener types for native controls (`Window`, `VStack`, `HStack`, `Button`, `Text`, etc.).
2. **Headless React / JSX Reconciler:** The platform-agnostic reconciler implementation that turns
   JSX trees into imperative widget mutations (`createInstance`, `appendChild`, `commitUpdate`).
3. **Platform OS C/Rust Glue:** The low-level bridging layer that interacts with OS UI APIs:
   - macOS / iOS: Objective-C / Swift bridging to `NSWindow`, `NSView`, and SwiftUI hosting controllers.
   - Linux: GTK4 C-FFI / `gtk-rs` bindings creating GTK application windows and widgets.
   - Windows: Win32 message dispatch and WinUI/XAML component wrappers.
4. **OS Utility Bridges:** Native file open/save dialogs, system clipboard integration, and
   system dark/light appearance queries.

#### Explicitly Rejected from Perry:
1. **NaN-Boxing & Value Model (`perry-runtime`):** Rejected. Kiln retains its 32-byte `kiln_value`,
   hidden-class shapes, and process-wide inline caches (ADR 0071). Mixing NaN-boxing into Kiln's runtime
   would violate cache and GC invariants.
2. **Direct LLVM / Cranelift Codegen (`perry-codegen`):** Rejected. Kiln retains its clean,
   cachable C99 emitter ([`kiln-backend-c`](file://~/Work/kiln/crates/native/kiln-backend-c)).
3. **SWC Compiler Core:** Rejected. Kiln retains its incremental query database ([`kiln-db`](file://~/Work/kiln/crates/compile/kiln-db)).

### 4. Main-Thread OS Runloop Integration

Native GUI frameworks require that the process's main thread own the OS event loop. This inverts
Kiln's standard Tokio/uring execution model:

* **The Inversion Seam:** When the `gui` capability is active, the host entry point initializes the
  platform application (`NSApplicationMain`, `g_application_run`, or Win32 message pump) on the
  main thread.
* **Turn Quantum Pumping (ADR 0070):** Kiln's microtask queue, asynchronous I/O completion callbacks,
  and timer events are scheduled onto the native runloop using platform-native idle/dispatch hooks:
  - macOS/iOS: `CFRunLoopPerformBlock` and `dispatch_async(dispatch_get_main_queue(), ...)`.
  - Linux: GLib's `g_idle_add` and `g_timeout_add`.
  - Windows: Custom window message posting (`PostMessage(hwnd, WM_KILN_TURN, ...)`).
* Worker threads (such as `kiln:actors` or background I/O tasks) communicate with the UI thread
  exclusively via message queues that wake the main runloop.

### 5. Headless Virtual Mock & Differential Parity

To uphold Kiln’s core invariant of dual-engine differential parity ([`scripts/differential.sh`](file://~/Work/kiln/scripts/differential.sh)):

* The interpreter ([`kiln-interpreter`](file://~/Work/kiln/crates/interp/kiln-interpreter))
  implements `kiln:gui` against an in-memory virtual DOM/widget tree.
* In automated tests, UI trees can be mounted, inspected, clicked, and verified in headless environments
  without opening real display windows.
* The native backend supports the same headless mode via a mock provider variant, ensuring that
  interactive GUI applications can be verified in automated CI gates.

## Consequences

### Positive
* **True Native GUI with Zero Webview Overhead:** Apps compile to single native binaries with
  near-instant startup (~3 ms) and modest memory usage (15–20 MB), avoiding Electron and Chromium.
* **Strict Compiler Isolation:** The compiler frontend and C99 codegen remain completely UI-agnostic.
  All UI logic lives behind `kiln:gui`, `HostOps`, and the capability-planned provider staticlib.
* **Zero Headless / Server Regressions:** Programs that do not use `kiln:gui` incur no binary size,
  build time, or runtime performance penalties (ADR 0062).
* **Full Mobile / App Store Viability:** Because Kiln compiles AOT to native C99, applications comply
  with Apple App Store and mobile platform policies prohibiting JIT compilation.

### Negative / Trade-offs
* **Platform Maintenance Surface:** Supporting native widgets across macOS, Linux, and Windows requires
  maintaining platform C/Objective-C/Rust glue code in the GUI provider crate.
* **Runloop Inversion Complexity:** Main-thread ownership must be carefully coordinated with Kiln’s
  actor scheduling and asynchronous Tokio tasks.

## Rejected Alternatives

* **Embed WebViews (Tauri / Electron model):** Rejected. Defeats the purpose of native controls,
  drastically inflates memory usage, and forfeits native widget accessibility and performance.
* **Direct LLVM Code Generation for UI (Perry model):** Rejected. Bakes OS UI dependencies into the
  compiler binary and slows compiler iteration.
* **Hardcode UI Primitives in Bytecode / HIR:** Rejected. Violates separation of concerns; all UI
  interactions must flow through standard module calls and the HostOps seam.
* **Monolithic Provider Archive:** Rejected. Violates ADR 0062; GUI capabilities must be split into
  an on-demand staticlib package.
