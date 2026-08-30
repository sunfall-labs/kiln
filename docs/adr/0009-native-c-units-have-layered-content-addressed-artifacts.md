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
layout, status codes, and HostOps callback. For each scalar build it composes those declarations
with the content-derived entry symbol under a module-specific include guard. The native coordinator
publishes that program header at `output.with_extension("h")` before atomically publishing the
shared library, then reports both paths over the versioned protocol. Library outputs ending in
`.h` are rejected, and coordination layers reject a derived header that would overwrite the source.
Each file publication is atomic, but the pair is not transactional; the library is the final ready
signal until a future versioned package-directory design can swap a manifest for the pair.

A shared-library build compiles the application unit as position-independent code and links only
that unit (plus required system libraries). PIC objects and shared links have cache identities
distinct from executable objects and links; an executable cache hit is never accepted as an
embedding artifact. Shared-library linking is initially supported on macOS and Linux, where the
toolchain flags, emitted header, and dynamic-loader behavior are exercised by real C-host
integration tests.

An acyclic module graph is embedded behind one topology-derived wrapper symbol with the same
two-argument entry ABI as a scalar shared library. The graph's generated module entries keep their
private import/export-array ABI. The wrapper allocates those arrays per invocation, evaluates
modules in dependency-first order, and links only their PIC objects plus its own PIC object: it
does not reuse the process-oriented CLI runtime or graph launcher. Module C and object cache keys
remain independently reusable, while the wrapper source/object keys include graph wiring and the
final shared-link key includes the ordered module objects, wrapper object, and platform library
identity.

The HostOps contract permits a callback-produced string to remain valid only until the current
module entry returns. A provider can export that value for a later consumer, so the graph wrapper
interposes a private HostOps proxy and copies each successful callback string before the upstream
callback returns. Those copies live through the whole graph invocation and are then released with
the wrapper's per-call storage. This solves provider-to-consumer lifetime and keeps concurrent
invocations isolated. It does not broaden ABI v1: callers still cannot assume an arbitrary string
in the public result remains valid after the wrapper returns. Retainable results or leak-free
caller ownership require a future versioned ownership/dispose or caller-buffer design.

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
- Multi-module embedding preserves dependency order without process-global import/export storage,
  so repeated and concurrent calls do not share graph execution state.
- Embedders consume the header emitted by the same build as the library, so a content-derived
  symbol change cannot silently leave a hand-written declaration stale.
- The platform compiler is a process boundary with captured diagnostics, while all source and
  artifact identity stays shell-independent and deterministic.
- The initial scalar C backend still uses `%.17g` for number-to-string conversion and retains
  temporary concatenation allocations for a process lifetime. Those are explicit equivalence and
  ownership gaps to close before claiming complete JavaScript number formatting or long-lived
  native embedding; they are not accepted as silent conformance success.
- A 128-bit deterministic module symbol is not the artifact integrity boundary. SHA-256 cache keys
  protect artifact reuse, and multi-module linking rejects any symbol collision before invoking
  the linker.
