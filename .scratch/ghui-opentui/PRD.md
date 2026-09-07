# Run ghui with OpenTUI

Status: in-progress

## Why

`../ghui` is the first application in this workspace that combines a large React/TypeScript
module graph, top-level await, dynamic imports, the Node adapter, and a native terminal renderer.
It now compiles to a native executable, but it does not yet run to its first frame.

## What is established

With the landing change, a clean ghui checkout builds successfully with the native-release profile. The
build generated about 816 MB of C, compiled it in parallel, and produced a 231 MB executable.

The same application exposed a module scheduler bug in the interpreter: a dynamic-import job was
queued on the interpreter's private realm while Node module execution drained the shared module
realm. The dependency remained suspended at top-level await and its importer observed an
uninitialised binding. The landing change queues that job through `with_realm_mut`; the focused module graph
regression and interpreter suite pass.

After that fix, interpreted ghui reaches OpenTUI's explicit platform check and reports:

    opentui is not supported on the current platform: kiln-undefined

The native executable currently loses the specific cause and reports only:

    kiln: the host refused an operation

## Boundary

OpenTUI 0.4.2 has no pure-JavaScript or headless renderer. Its test renderer also constructs the
native renderer. Production selects a platform package using `process.platform` and
`process.arch`, loads the package's shared library through `node:ffi` or `bun:ffi`, registers
same-thread callbacks, and passes pointers and array buffers across the boundary. Its library
definition declares 327 symbols using scalar and pointer arguments.

Pretending that a Kiln realm is Node on Linux or macOS is not a fix. `process.platform ===
"kiln"` is deliberate: advertising a host-native contract that the realm does not implement
would merely move the failure into a less legible branch.

## Outcome

ghui should render and accept terminal input from a Kiln native executable through an explicit,
tested native-provider contract. Unsupported host operations must retain a diagnostic naming the
operation that was refused.
