# Runtime-generated ESM in native realms

## Status

First implementation slice verified. The native graph can execute a bounded subset of ESM files
created while the program is running; Effect's Schema AOT tests exercise this path. Full ECMAScript
module grammar remains outside this slice.

## User-visible behavior

In a Node-compatible native run, `await import(fileURL)` for an authorized source file resolves to
the same module namespace and values the current realm would use for an ordinary ESM import. The
module runs once per canonical URL per realm. Syntax, resolution, linking and evaluation failures
reject the import promise with the appropriate error. Imported values preserve identity with
already-linked modules in the native graph.

## Ownership

Filesystem access and module resolution are host capabilities. The active native realm owns the
module cache, module environments, live export cells and namespaces. Generated code executes in
the existing native realm's value kernel and module instances. The existing native dynamic source
evaluator is the execution path for the first admitted source subset; unsupported syntax is an
explicit promise rejection. A second interpreter heap, a schema-specific recognizer, bundling, or
ambient compiler subprocess is not an implementation of this contract.

## First implementation slice

Admit source modules created after startup whose dependencies resolve to modules already linked
into the active native graph. Parse them with module goal and strict mode, instantiate lexical
bindings/import cells before evaluation, cache by canonical URL, and expose a frozen namespace
whose exports read live cells. Keep the existing graph linker as the authority for dependencies
that are already compiled, including package export conditions and namespace identity. Runtime
specifier resolution uses a build-embedded registry derived from the resolver's actual package
exports and canonical linked module paths; package names and paths are never hard-coded. Reject
unsupported source syntax or dependency shapes explicitly; never return an empty or partial module
namespace.

The generic native probe covers runtime creation and import of a module that imports a linked graph
target, exports a function, preserves namespace/cache identity, and reports missing-module and
syntax failures. The pinned Effect Schema JIT/AOT native suite passes all 129 cases without changing
upstream assertions. The CLI now transports resolver-approved aliases for synchronized targets in
the resolution-map identity; this preserves lazy loading and cannot add undiscovered source files.

## Open design point

The generated-C executable does not carry Oxc at runtime. The native dynamic evaluator is a
same-realm execution engine with bounded grammar; it must admit only syntax it can parse and
execute correctly. This slice does not claim full ECMAScript module grammar. Every syntax or
linking feature outside the tested module subset remains an explicit unsupported-import rejection
until its semantics are implemented.
