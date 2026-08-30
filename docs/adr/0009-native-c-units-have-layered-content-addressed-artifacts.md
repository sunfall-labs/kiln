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
validates bytecode version, control flow, indices, and stack shape before emission. Validation is
closed-world across reachable and unreachable code: constants, locals, stack operations, scalar
unary and binary operators, jumps, `HostCall::WriteStdout`, and `Return` are accepted, while every
other instruction is rejected before C rendering. `In` and `InstanceOf` remain unsupported because
the scalar ABI has no identity-bearing references. The same boundary applies to functions,
closures, objects, properties, and the other reference-based instructions until native values gain
a heap representation. Every accepted scalar operator has executable JavaScript-result tests and
native/interpreter equivalence coverage; the C dispatcher's default arm is not a semantic fallback.

The backend's canonical program identity includes the complete ordered function-prototype table as
well as top-level code:
function metadata, instruction streams, and tagged upvalue sources all participate, while numeric
`MakeFunction` references remain flat so recursive function graphs cannot recurse during hashing.
This identity rule lands before callable functions reach C emission, preventing that future backend
from reusing an object after only a nested function changed. The translation unit calls the numeric
HostOps callback and contains no stdout, process, or command-line behavior.

The command-line runtime is a separate, program-independent C translation unit. A tiny generated
launcher binds that runtime to one standalone module or an ordered linked graph. This split lets an
application object survive a runtime implementation change and lets the runtime object be reused
across applications. Linked modules compile independently, while the launcher owns dependency
ordering and import/export-array wiring.

`kiln-artifact-cache` stores typed records addressed by SHA-256 keys. Keys include artifact kind,
compiler/backend identity, HostOps/runtime ABI, target, CPU policy, build profile, compatibility
mode, options digest, and ordered input digests. Records also contain and verify the key and payload
digests. Lookups stop on corruption rather than disguising it as a miss. Cache records use a
same-directory temporary file and no-replace hard-link installation; layers are searched in order
and only explicitly writable layers receive artifacts. Materialized native outputs use
same-directory temporaries and atomic replacement, retrying unique names so a temporary abandoned
by a crashed, PID-reused process cannot block a later build.

`kiln-native-toolchain` discovers Zig, Clang, a generic `cc`, or GCC, retains the canonical compiler
path, version, family, and compiler-reported target, and exposes captured structured compile/link
failures. `native-dev`, `native-check`, and `native-release` select distinct flags. Zig is preferred
when installed, but it is not a mandatory bootstrap dependency; a compatible system compiler is a
supported fallback. Unix links request the system math library because generated exponent and
remainder operations use `pow` and `fmod`.

The compiler identity also hashes the inherited search-policy variables relevant to the selected
driver and C11 language mode. Every driver includes `CPATH`, `C_INCLUDE_PATH`, and `LIBRARY_PATH`;
GCC and an unidentified `cc` additionally include `COMPILER_PATH` and `GCC_EXEC_PREFIX`, while Zig
includes `ZIG_LIB_DIR`. Names and raw platform values are length-framed, and unset remains distinct
from empty. `SDKROOT` and `MACOSX_DEPLOYMENT_TARGET` remain part of target identity; variables for
other source languages and shell conventions such as `CFLAGS` and `LDFLAGS` are not inputs because
Kiln compiles C11 directly and does not interpret those conventions. Changing applicable compiler
search policy therefore misses object and link artifacts instead of reusing an artifact built
against a different inherited environment.

On macOS, a Zig-selected build uses the selected Zig driver for C compilation but uses the host
Apple clang/ld64 driver for every shared-library final link (scalar embedding libraries, graph
embedding libraries, and the dynamically loaded CLI runtime). Zig 0.16 accepts but does not
enforce the Mach-O `exported_symbols_list` policy when a reachable archive contributes symbols;
silently retaining Zig for the final link would therefore leak private Rust/provider symbols.
The actual link-driver identity is included in loader and linked-artifact identities and is the
tool reported for shared-library builds. Compile-object identities remain keyed by the selected
compiler, including the macOS Zig `NativeDev` suppression of Zig's implicit UBSan instrumentation
when those objects are finalized by clang; `NativeCheck` retains its explicit sanitizer flags. The
graph CLI runtime has a distinct export set and cache identity because it also
exports the private scheduler entry points needed by graph launchers.

The cache coordinator treats generated module C, application object, runtime object, launcher
object, and linked executable as separate artifacts. Cache records are containers rather than
direct linker inputs, so validated payloads are materialized into process-owned work paths. A cached
deterministic source or bundled-provider payload must also equal the bytes regenerated or shipped
by the current process; a self-consistent record under the right key but with different bytes is
corruption rather than a hit. A cached
executable is materialized with executable permissions at the caller's requested output path.
Independent application, runtime, and launcher units compile concurrently on a cold build. Native
runs launch a process-owned executable before publishing the stable output path, so concurrent
requests cannot replace the program another request is about to execute. The CLI launcher pins the
C numeric locale before module initialization as an additional defence against locale-dependent
parsing.

Watch mode remains interpreter-first: each `Run` response carries the database revision whose
source state it executed, and native refresh uses that revision on a second daemon connection. The
daemon prepares only a matching snapshot, compiles outside the engine lock to a request-unique
sibling output, then reacquires the lock and atomically publishes only if the revision is still
current. A stale success or failure removes its temporary output and is reported as superseded.
The CLI permits one active native compile and one overwritable latest pending request, so edits do
not block interpretation or create an unbounded build queue. This is logical cancellation: an
obsolete toolchain process may finish, but it cannot publish.

Generated modules cannot assume that policy when embedded in a host process. ADR 0014 therefore
replaces the interim `strtod_l` path with a bounded, locale-free decimal parser and an explicit
ECMAScript grammar/radix adapter. Number formatting likewise does not use libc locale or `printf`:
ADR 0013 delegates it to the separately versioned, Ryu-derived native number runtime. Both now
compose into one cached hidden number-conversion object.

Embedding uses the same module entry ABI without linking the command-line runtime or launcher.
`kiln-native-runtime` publishes a deterministic, self-contained C header for the versioned value
layout, status codes, and HostOps callback. For each scalar build it composes those declarations
with the content-derived entry symbol under a module-specific include guard. The native coordinator
publishes that program header at `output.with_extension("h")` before atomically publishing the
requested shared-library copy, then reports those paths and the canonical loader path over the
versioned protocol. Library outputs ending in `.h` are rejected, and coordination layers reject a
derived header that would overwrite the source.
Each file publication is atomic, and a process mutex plus an advisory lock on the output directory
prevents concurrent builders from interleaving different shared packages. The package is still not
reader-transactional; the requested library path is the final ready signal until a future versioned
package-directory design can swap a manifest for the package. Directory scope is required because
different library suffixes can derive the same header pathname.

Default executable and shared-library paths preserve the entry's complete project-relative source
filename before appending a platform artifact suffix. Thus `src/main.ts` and `src/main.js` publish
as distinct artifacts just as `src/main.ts` and `tests/main.ts` do; an extension is part of source
identity and must not be discarded by the publication mapping. Explicit output paths remain exact.

A shared-library build compiles the application unit and internal native number runtime as
position-independent code, then links those units plus required system libraries. PIC objects and
shared links have cache identities distinct from executable objects and links; an executable cache
hit is never accepted as an embedding artifact. Shared-library linking is initially supported on
macOS and Linux, where the toolchain flags, emitted header, and dynamic-loader behavior are
exercised by real C-host integration tests.

The dynamic-loader identity is a canonical, slash-free file under the output directory's reserved
`.kiln-loaders` namespace. Its name contains the full SHA-256 digest of the pre-link compiler,
backend kind, embedding ABI, target, profile, link policy, public symbol, and ordered object/runtime
inputs. Mach-O records `@rpath/<canonical leaf>` and ELF records the canonical leaf as `DT_SONAME`;
the namespace is not part of either embedded identity because a slash-bearing ELF SONAME bypasses
RPATH search. The builder publishes that canonical file first, the header second, and a byte-for-byte
copy at the exact requested output path last. It reports all three paths. The loader namespace is
reserved against explicit outputs, and an existing content name is never overwritten with different
bytes. The loader namespace and each loader leaf must be real directories/files rather than
symlinks or other filesystem objects. A missing leaf is installed by atomically hard-linking a
fully written temporary inode, which cannot replace a concurrent winner; an existing winner is
accepted only after a byte-for-byte check. Embedders must package the canonical loader file and add
its directory to RPATH; the requested path remains a stable human alias and publication-ready
signal. Distinct content therefore cannot collapse merely because two requested paths share a
basename, while identical linked content can safely share one loader identity. Old canonical
generations remain until an explicit future cache/package cleanup policy; an in-use loader is never
eagerly deleted during rebuild.

Manifest-selected native runtime inputs use the same package confinement rule at their distribution
boundary: the manifest and its single-component sibling archive must be regular files. Digest-valid
bytes behind a symlink are rejected rather than silently making an installation depend on a path
outside the package.

An acyclic module graph is embedded behind one content-derived public wrapper symbol with the same
two-argument entry ABI as a scalar shared library. Its strong identity covers graph topology and
the ordered generated module sources, so two loaded builds with identical paths and wiring but
different bodies cannot interpose one public wrapper onto the other. The graph's generated module
entries keep their private import/export-array ABI and path-derived identities, so their C and
object cache keys remain independently reusable. The wrapper allocates import/export arrays per
invocation, evaluates modules in dependency-first order, and links their PIC objects, one shared
native-number object, and its own PIC object: it does not reuse the process-oriented CLI runtime or
graph launcher. Wrapper source/object keys include the public graph identity and wiring, while the
final shared-link key includes the ordered module objects, wrapper object, and platform library
identity.

The private graph-module ABI is also private at dynamic-loader scope. Graph module objects compile
with hidden symbol visibility while the public wrapper retains default visibility. Two loaded
libraries may contain the same logical module path with different bodies; hiding the path-derived
six-argument entries prevents ELF interposition (and keeps the same invariant on macOS) without
changing generated C or relying on a platform-specific symbolic-binding linker option. The
visibility policy is part of each graph module object's cache identity. Scalar shared entries stay
public because their content-derived two-argument symbol is the scalar embedding API.

The HostOps contract permits a callback-produced string to remain valid only until the current
module entry returns. A provider can export that value for a later consumer, so the graph wrapper
interposes a private HostOps proxy. After the upstream callback returns, while ABI v1 still
requires its result string to be alive, the proxy copies that string before it returns control to
the generated module. A callback-local stack buffer is not a valid result: its lifetime has ended
before the proxy can inspect it. The copies live through the whole graph invocation and are then
released with the wrapper's per-call storage. This solves provider-to-consumer lifetime and keeps
concurrent invocations isolated. It does not broaden ABI v1: callers still cannot assume an
arbitrary string in the public result remains valid after the wrapper returns. ADR 0030 adds an
optional ABI-v2 descriptor whose opaque result handle owns a normalized scalar snapshot and
exposes library-owned disposal; the legacy entry and its borrowed-result contract remain
unchanged.

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
- Multiple graph libraries can be loaded into one process without one library's path-derived
  module entries replacing another's; only each graph's documented wrapper is externally visible.
- Embedders consume the header emitted by the same build as the library, so a content-derived
  symbol change cannot silently leave a hand-written declaration stale.
- The platform compiler is a process boundary with captured diagnostics, while all source and
  artifact identity stays shell-independent and deterministic.
- Number formatting and string parsing are ECMAScript-spelled through the versioned native number
  runtime. ADR 0030 makes the public scalar result lifetime explicit, but temporary concatenation
  allocations are still retained for a process lifetime; that internal ownership gap remains
  before claiming a leak-free realm heap.
- A 128-bit deterministic module symbol is not the artifact integrity boundary. SHA-256 cache keys
  protect artifact reuse, and multi-module linking rejects any symbol collision before invoking
  the linker.
