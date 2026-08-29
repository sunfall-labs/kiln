# ADR 0009: Native C units have layered content-addressed artifacts

## Status

Accepted.

## Context

The founding design calls for native development builds without making every source edit pay for
whole-program code generation, runtime compilation, and linking. It proposes C as an initial
portable backend and Zig/LLD as a convenient toolchain, but Kiln also needs to work on developer
machines and CI runners that provide an ordinary Clang or GCC-compatible `cc` driver. Native and
interpreter execution must share bytecode semantics and HostOps rather than grow separate source
frontends or embed command-line I/O policy in application objects.

A filesystem path is not a sufficient cache identity. Generated code also depends on the bytecode,
backend and ABI versions; objects depend on the compiler identity, target, flags, and generated C;
links depend on ordered object contents and linker policy. Reusing an artifact without those inputs
would be fast but unsound.

## Decision

`kiln-backend-c` deterministically emits one C translation unit and stable external symbol per
bytecode module. Standalone units retain a content-derived symbol. A linked graph instead assigns
each unit a symbol derived from its normalized project-relative logical path, never from an
absolute checkout path. The graph rejects any symbol collision before invoking the linker. This
distinction lets a body-only edit change the module C/object keys without changing the launcher
source that names the module. Artifact integrity and ABI compatibility remain content-addressed in
the versioned native cache keys; a symbol is a link identity, not a cache proof. The backend
validates bytecode version, control flow, indices, and stack shape before emission. The translation
unit calls the numeric HostOps callback and contains no stdout, process, or command-line behavior.

The command-line runtime is a separate, program-independent C translation unit. A tiny generated
launcher binds that runtime to one standalone module or an ordered linked graph. This split lets an
application object survive a runtime implementation change and lets the runtime object be reused
across applications. Linked modules compile independently, while the launcher owns dependency
ordering and import/export-array wiring.

`kiln-artifact-cache` stores typed records addressed by SHA-256 keys. Keys include artifact kind,
compiler/backend identity, HostOps/runtime ABI, target, CPU policy, build profile, compatibility
mode, options digest, and ordered input digests. Records also contain and verify the key and payload
digests. Lookups stop on corruption rather than disguising it as a miss. Writes use a same-directory
temporary file and atomic rename; layers are searched in order and only explicitly writable layers
receive artifacts.

`kiln-native-toolchain` discovers Zig, Clang, a generic `cc`, or GCC, retains the canonical compiler
path, version, family, and compiler-reported target, and exposes captured structured compile/link
failures. `native-dev`, `native-check`, and `native-release` select distinct flags. Zig is preferred
when installed, but it is not a mandatory bootstrap dependency; a compatible system compiler is a
supported fallback. Unix links request the system math library because generated exponent and
remainder operations use `pow` and `fmod`.

The cache coordinator treats generated module C, application object, runtime object, launcher
object, and linked executable as separate artifacts. Cache records are containers rather than
direct linker inputs, so validated payloads are materialized into process-owned work paths. A cached
executable is materialized with executable permissions at the caller's requested output path.
Independent application, runtime, and launcher units compile concurrently on a cold build. Native
runs launch a process-owned executable before publishing the stable output path, so concurrent
requests cannot replace the program another request is about to execute. The CLI launcher pins the
C numeric locale before module initialization so `strtod` and `snprintf` do not inherit
locale-dependent semantics.

Embedding uses the same module entry ABI without linking the command-line runtime or launcher.
`kiln-native-runtime` publishes a deterministic, self-contained C header for the versioned value
layout, status codes, and HostOps callback. A shared-library build compiles the application unit as
position-independent code, links only that unit (plus required system libraries), and returns its
stable module symbol for the embedder to declare and call. PIC objects and shared links have cache
identities distinct from executable objects and links; an executable cache hit is never accepted as
an embedding artifact. Shared-library linking is initially supported on macOS and Linux, where the
toolchain flags and dynamic-loader behavior are exercised by real C-host integration tests.

## Consequences

- Editing one module body invalidates that module's C and object plus the final link, not the
  runtime, graph launcher, or unrelated application objects. Moving or renaming the logical module
  intentionally changes its linker identity.
- Changing HostOps or the native runtime invalidates exactly the artifacts whose ABI or contents
  changed.
- Compiler versions, targets, profiles, and ordered link inputs cannot accidentally share objects.
- Native build reports can expose cache hits, artifact sizes, and per-stage latency as ordinary
  correctness/performance evidence.
- Native embedding keeps ownership and I/O policy in the host process: the library exports module
  computation, while every external effect still crosses the same numeric HostOps callback.
- The platform compiler is a process boundary with captured diagnostics, while all source and
  artifact identity stays shell-independent and deterministic.
- The initial scalar C backend still uses `%.17g` for number-to-string conversion and retains
  temporary concatenation allocations for a process lifetime. Those are explicit equivalence and
  ownership gaps to close before claiming complete JavaScript number formatting or long-lived
  native embedding; they are not accepted as silent conformance success.
- A 128-bit deterministic module symbol is not the artifact integrity boundary. SHA-256 cache keys
  protect artifact reuse, and multi-module linking rejects any symbol collision before invoking
  the linker.
