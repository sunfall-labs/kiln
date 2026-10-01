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
* High-level declarative frameworks sit entirely in userland or standard npm packages. Rather than
  inventing a bespoke proprietary UI framework, Kiln directly targets official **`solid-js/universal`**
  (`createRenderer`). The native built-in `kiln:gui` exports the canonical 10-method universal host
  driver (`universalDriver`: `createElement`, `createTextNode`, `isTextNode`, `replaceText`, `insertNode`,
  `removeNode`, `setProperty`, `getParentNode`, `getFirstChild`, `getNextSibling`) as well as pre-wired
  Solid JSX primitives (`render`, `createElement`, `createSignal`, `createMemo`, `createEffect`).
  Developers can either point their JSX compiler (`babel-preset-solid` / Oxc / Vite) directly at `kiln:gui`
  for zero-install native desktop apps, or import `createRenderer` from official `solid-js/universal`
  with Kiln's host driver (`createRenderer(universalDriver)`). Because Solid executes components only
  once and binds fine-grained reactive signals directly to native node properties, it operates with zero
  Virtual DOM overhead, zero tree diffing, and negligible GC allocation pressure during user interactions.
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
2. **Declarative Component Paradigms:** The widget prop models, event binding patterns, and
   tree mutation mappings adapted into SolidJS 2.0's 10-method `createRenderer` contract.
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

### 4. Main-Thread OS Runloop Integration: The Turn Mailbox & Wakeup Seam

Native GUI frameworks strictly require that the process's main thread (thread 0) own the OS event
loop. Synchronizing an asynchronous runtime with a native OS runloop often introduces deadlocks,
reentrancy bugs, or battery-draining spin loops. Kiln avoids these failure modes by making the OS
runloop the authoritative host, treating Kiln's execution as discrete turn quanta (ADR 0070), and
connecting them via an **Asynchronous Turn Mailbox**.

#### The Three Structural Invariants:
1. **The UI Realm is Pinned to the Main Thread (No UI Mutexes):**
   Because operating systems mandate that UI widgets must only be created and mutated on thread 0,
   the JavaScript realm that owns the UI is pinned to the main thread. It never locks UI widgets
   behind mutexes. All widget mutations occur synchronously during a turn on the main thread,
   rendering cross-thread UI deadlocks structurally impossible.
2. **Pure Asynchronous Mailbox (Zero Synchronous Cross-Thread Waits):**
   Deadlocks in GUI runtimes occur when the main thread blocks waiting for a background thread while
   the background thread blocks waiting for the main thread. Kiln enforces a strict unidirectional
   rule:
   - The main thread **never** synchronously waits on background workers or I/O. All operations
     settle as asynchronous Promises.
   - Background threads (Tokio I/O, `kiln:actors` under ADR 0072) **never** synchronously block
     waiting for the main thread. When background work settles, it enqueues a completion message into
     the main thread's lock-free Turn Mailbox and signals the OS runloop.
3. **Reentrancy Guard (Preserving Run-to-Completion):**
   If a native widget mutation (e.g. `win.setTitle(...)`) causes the OS to fire an immediate synchronous
   native notification (e.g. window resize/repaint) while JavaScript is still on the stack, executing
   the JS event handler immediately would violate ECMAScript's run-to-completion invariant and risk
   stack overflow. Kiln intercepts synchronous native callbacks during an active turn and enqueues
   them into the Turn Mailbox to be executed as subsequent, clean turns after the current stack unwinds.

#### Platform-Native Zero-CPU Wakeup Primitives:
When no turns or UI events are active, the main thread idles inside the kernel wait primitive at 0%
CPU. When background actors or I/O completions arrive, they awaken the main thread immediately
without polling:

| Platform | Native Runloop Host | Zero-CPU Idle Wait | Cross-Thread Wakeup Primitive |
| :--- | :--- | :--- | :--- |
| **macOS / iOS** | `CFRunLoop` / `NSApplication` | `mach_msg` | `CFRunLoopSourceSignal(source)` + `CFRunLoopWakeUp(mainLoop)` |
| **Linux** | `GMainContext` (GTK4) | `poll()` / `epoll()` on display fd | `g_main_context_wakeup(context)` or 8-byte write to `eventfd` |
| **Windows** | Win32 Message Pump | `GetMessage()` / `MsgWaitForMultipleObjects` | `PostMessage(hwnd, WM_KILN_TURN, 0, 0)` |

Each tick of the runloop drains the pending mailbox messages, executes one turn quantum (ADR 0070),
flushes the microtask queue, updates native widget properties via HostOps, and returns control to the
OS runloop to sleep.

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
* **Virtual DOM / React Reconciler Model (`react-reconciler`):** Rejected. VDOM diffing forces
  repeated component execution and fiber allocations, creating continuous GC pressure during typing,
  scrolling, and interactive events. SolidJS 2.0's fine-grained signals execute components once and
  update scalar properties directly with near-zero GC allocations, while sharing frontend compiler
  architecture with Kiln via Oxc.
* **Monolithic Provider Archive:** Rejected. Violates ADR 0062; GUI capabilities must be split into
  an on-demand staticlib package.
