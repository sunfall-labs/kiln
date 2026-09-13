# Run ghui with OpenTUI

Status: in-progress; live acceptance is not verified

## Why

`../ghui` is the first application in this workspace that combines a large React/TypeScript
module graph, top-level await, dynamic imports, the Node adapter, and a native terminal renderer.
It now compiles to a native executable, but it does not yet run to its first frame.

## What is established

The current full build at integration `1dbb6fd0` generated 161,313,202 bytes of C and a
112,237,608-byte executable. It reaches the splash path and then currently exits with SIGSEGV
during live diagnosis. Earlier mock sessions reached the first frame and accepted navigation;
that is historical evidence, not current live acceptance.

The same application exposed a module scheduler bug in the interpreter: a dynamic-import job was
queued on the interpreter's private realm while Node module execution drained the shared module
realm. The dependency remained suspended at top-level await and its importer observed an
uninitialised binding. The landing change queues that job through `with_realm_mut`; the focused module graph
regression and interpreter suite pass.

The explicit `kiln:process` capability is now implemented for both execution engines. Its public
surface is:

    run(command, args?, { input?, signal? }) -> Promise<{ stdout, stderr, exitCode, signal }>

Type and option validation, binary length-framed request encoding, AbortSignal cleanup, and result
shaping live in the TypeScript facade. Tokio owns process spawning, pipes, status collection, and
kill-and-wait cancellation. The native bridge accepts only normalized request bytes and returns a
rooted completion record; it does not duplicate public validation or object shaping. Realm shutdown
drains process cleanup tasks before aborting unrelated host work.

Permanent parity coverage is `conformance/native/process-run.cjs`, executed by `scripts/native.sh`;
the focused checks are `mbx test -p kiln-runtime --offline` and
`mbx test -p kiln-native-number-ffi --offline`. The fixture covers binary input containing NUL,
stdout/stderr, nonzero exit status, signal nullability, and AbortSignal cancellation in interpreter
and generated native C execution.

The read-only real process boundary probe is established: the native capability imports and runs a
real command, returning 1,084 bytes of stdout and a successful schema result in 1.23 seconds
(binary hash `41f697...`). No GitHub mutation was performed. The live ghui first-frame and input
acceptance remain pending.

The remaining renderer work is tracked separately. Earlier probes reached OpenTUI's explicit
platform check and reported:

    opentui is not supported on the current platform: kiln-undefined

Some earlier binaries lost the specific cause and reported only:

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
