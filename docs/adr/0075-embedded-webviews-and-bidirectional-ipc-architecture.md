# ADR 0075: Embedded WebViews, Custom Protocol Routing, and Bidirectional IPC Architecture

## Status

Proposed, 2026-09-29.

Follows ADR 0062 for demand-driven provider capabilities, ADR 0068 for kiln extension modules,
ADR 0070 for realm turns and schedulable quanta, and ADR 0074 for native GUI capability and host
runloop integration.

## Context

ADR 0074 establishes Kiln's native desktop capability (`kiln:gui`), providing native widgets (GTK4,
AppKit, WinUI) driven by SolidJS 2.0 fine-grained reactivity with sub-10ms startup and zero
Chromium/Node.js baggage.

While native controls provide optimal memory, speed, and OS accessibility for standard desktop
interfaces (forms, toolbars, buttons, dialogs, tray icons), many production applications require
rich document rendering, canvas/WebGL visualizations, code editors (Monaco), or hybrid migration of
existing web frontends.

Historically, desktop runtimes forced an all-or-nothing dichotomy:
1. **Electron:** Bundles a complete multi-hundred-megabyte Chromium engine and a separate Node.js
   runtime per window, consuming 150–300 MB of RAM at idle and introducing substantial cold-boot
   latency.
2. **Tauri / Wry:** Employs the operating system's native web engine (WebKitGTK on Linux, WKWebView
   on macOS, WebView2 on Windows), slashing executable size and memory usage.

For Kiln, embedding web views introduces distinct architectural challenges that must align with
Kiln's core tenets: deterministic engine parity between bytecode interpreter and compiled C99,
unblocked reactive turns, memory safety, and minimal binary footprint.

## Decision

### 1. Architectural Principles

1. **Host-Owned Platform WebView (`wry` / OS Engine):**
   Kiln will not bundle Chromium or WebKit source. Instead, the native host GUI provider crate
   (`kiln-host-gui`) leverages the platform's native web engine via the `wry` ecosystem
   (WebKitGTK 4.1 on Linux, WKWebView on macOS, WebView2 on Windows).
2. **Demand-Driven Inclusion (ADR 0062):**
   Web view capabilities are isolated in `kiln-host-gui` under an explicit feature flag or capability
   descriptor. Programs that use native controls alone (`Window`, `Button`, `VStack`, `Menu`) never
   link WebKit or WebView2 libraries.
3. **Turn-Safe Main-Thread Execution:**
   Platform web views strictly require creation and manipulation on the primary OS thread. The Kiln
   host runloop (`runLoop` / `turnMailbox`) pumps platform events during the event drain cycle,
   ensuring IPC messages from the webview arrive as discrete mailbox turns without preempting active
   computations.
4. **Pure In-Memory Protocol Routing (`kiln://`):**
   Assets and virtual endpoints are resolved through registered custom URI scheme handlers directly
   in memory. No local HTTP/TCP servers are spawned, eliminating port collision risks and network
   sandbox bypasses.

### 2. The `WebView` Widget Primitive in `kiln:gui`

In `kiln:gui`, `WebView` is a first-class `Node` primitive that can be placed inside any container
(`Window`, `VStack`, `HStack`):

```tsx
import { Window, VStack, WebView, createSignal } from "kiln:gui";

function App() {
  const [url, setUrl] = createSignal("kiln://app/index.html");

  return (
    <Window title="Hybrid App" width={1024} height={768}>
      <VStack>
        <WebView
          src={url()}
          devtools={true}
          onMessage={(event) => {
            console.log("Message from DOM:", event.data);
          }}
          ref={(wv) => {
            wv.postMessage({ type: "init", payload: 42 });
          }}
        />
      </VStack>
    </Window>
  );
}
```

### 3. Bidirectional IPC Channel

The IPC layer bridges the guest JavaScript context running inside the webview and the host Kiln
runtime:

#### DOM-to-Host (Webview -> Kiln)
Inside the webview DOM, a global `window.__kiln` interface is injected prior to document execution:

```js
// Injected into webview DOM
window.__kiln.postMessage({ action: "saveFile", content: "..." });
```

The native webview platform handler catches the message and forwards it via `turnMailbox.post()`:

```text
[WebKitGTK / WKWebView / WebView2]
               │ (native script message handler)
               ▼
   [kiln_gui_webview_post_message]
               │ (C FFI boundary)
               ▼
         [turnMailbox]
               │ (reentrancy-safe turn queue)
               ▼
 [WebView.dispatchEvent("message", data)]
               │
               ▼
   [Kiln Reactive Listener]
```

#### Host-to-DOM (Kiln -> Webview)
The Kiln runtime evaluates scripts or dispatches structured events into the webview via native
script evaluation:

```ts
webView.postMessage({ action: "updateState", data: { count: 10 } });
// Equivalent to webview.evaluateJavascript(`window.dispatchEvent(new CustomEvent('kiln-message', { detail: ... }))`);
```

### 4. Custom Scheme Protocol (`kiln://`)

To securely serve local HTML, CSS, JavaScript bundles, and static assets:
1. `wry::WebViewBuilder::with_custom_protocol("kiln", ...)` intercepts all requests with scheme `kiln://`.
2. Resolves paths against a registered virtual asset directory or module bundle map.
3. Sets explicit Content-Type headers (`text/html`, `application/javascript`, `image/svg+xml`) and
   standard security headers (`Content-Security-Policy: default-src 'self' 'unsafe-inline' kiln:`).
4. Forbids `file://` access entirely, enforcing clean origin isolation.

### 5. Parity & Headless Degradation

In headless environments (CLI, CI conformance runs, containers lacking display servers):
- `new WebView()` behaves as an abstract `Node` with valid properties (`src`, `devtools`).
- `postMessage` queues messages virtualized in memory.
- Conformance tests verify DOM bridge serialization and IPC handler binding deterministically
  across both the bytecode interpreter and the compiled native C backend.

## Consequences

### Positive
- Allows hybrid applications mixing high-performance native controls with rich web content.
- Zero extra runtime binary download: leverages the OS-provided WebKit/WebView2 engine.
- High security: custom scheme interceptor avoids exposed loopback ports.
- Seamless SolidJS 2.0 reactivity integration via scalar property updates.

### Negative / Trade-offs
- WebKitGTK / WebView2 differences across Linux, macOS, and Windows can exhibit minor CSS/DOM
  rendering variances across platforms.
- Linux systems without WebKitGTK installed require installing `libwebkit2gtk-4.1` (standard on
  GNOME/Ubuntu/Fedora, but absent on minimal server distributions).
