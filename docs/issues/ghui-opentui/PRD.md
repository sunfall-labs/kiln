# Run ghui with OpenTUI

Status: live acceptance passed; follow-up work remains

## Why

`../ghui` is the first application in this workspace that combines a large React/TypeScript
module graph, top-level await, dynamic imports, the Node adapter, and a native terminal renderer.
It now has a verified live native session; the remaining work is to make slow external commands
diagnosable and predictable.

## What is established

The verified app source fixture is `afd00085f5aa925bb8220a091f932b1bc21e1ac8`, using adapter
`9484c3f`; it was built with compiler production revision `66d07dd9` from integration
`01aaaca5`. Binary SHA-256 is
`e112a489242695ad94a14bb6b16f1f47e96a9243792ebe88a8399fe8d6c7fefc` (112,840,824 bytes).

The latest production rerun used compiler revision `fea2802b` and binary SHA-256
`c4ca4c0b36d2f24d533fba8119ac25ce1e56a13a886223246890c6c687cdbe0e` (112,841,320 bytes).
It repeated the live read-only flow successfully and showed no timeout notice. The earlier
`66d07dd9` run and its bounded timeout finding remain historical evidence for the observability
follow-up below.

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

The read-only real process boundary probe remains established. The live process session also passed:
real account data rendered (16 repositories, 50+ pull requests, 15 issues), PR #3 details opened,
markdown details scrolled, Escape returned to the list, `q` restored the terminal, and the process
exited 0. No GitHub mutation was performed.

The session exercised the timer payload/keep-alive fix. Release regression coverage was run in the
normal configuration and with GC stress. The earlier `66d07dd9` run showed a transient
`Timed out after 15000ms` footer while entering PR details; its transcript cannot identify the exact
request because it has no structured command IDs or timestamps. The latest `fea2802b` rerun showed no
timeout notice. The bounded finding remains classified as an app/network budget issue rather than
evidence of a Kiln runtime defect.

The local follow-up issue is `docs/issues/ghui-opentui/issues/06-ghui-command-timeout-observability.md`.

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

ghui renders and accepts terminal input from a Kiln native executable through an explicit, tested
native-provider contract. Unsupported host operations retain diagnostics naming the refused
operation. The overall repository gate is a separate, still-running result and is not implied by
this acceptance.
