# ADR 0011: Module linkage is explicit, filesystem-scoped, and acyclic in the first scalar slice

## Status

Accepted.

## Context

The founding design makes the module graph a first-class query boundary. The first executable
module slice needs to resolve real source files, preserve import/export meaning across the parser
boundary, and evaluate more than one module without turning an import edge into an accidental
recursive database query. It also needs to retain the daemon's editor-overlay behavior: a graph
build must be able to consume already memoized source/HIR rather than rereading or reparsing every
file.

Oxc 0.147 already computes ECMAScript module records. Its `ImportEntry` records distinguish named,
default, and namespace imports, and its `ExportEntry` records distinguish local exports, indirect
re-exports, star exports, namespace re-exports, default names, and TypeScript type-only entries.
Keeping only a list of strings would lose the binding origin needed for linking and would make a
later live-binding implementation impossible to add without changing the frontend contract.

The execution surface remains deliberately smaller than the module grammar. The linker evaluates
dependencies before their importers, and its namespace objects are snapshots rather than exotic
live-binding views. Cycles therefore need a user-facing structured result while the representation
is still being prepared for ECMAScript's instantiate-then-evaluate semantics.

## Decision

### Oxc linkage ends in owned HIR

`frontend-oxc` is the only crate that sees Oxc types. It lowers the complete static linkage record
into `kiln-hir`: requested-module occurrences retain statement and specifier spans, request kind,
and type-only status; imports retain source, imported name, local name, both spans, and type-only
status; exports retain their visible name, origin, source where applicable, spans, and type-only
status.

The owned origin is intentionally not a nullable source string. Local bindings, default-expression
exports, named re-exports, star exports, namespace re-exports, and parser-recognized unsupported
forms remain distinct until graph linking. Type-only entries remain in HIR for future type
checking but are removed from runtime bindings and runtime dependency evaluation.

Default-expression exports have an explicit executable HIR statement. This prevents an expression
from being executed and discarded as an ordinary expression statement while its export metadata
claims a runtime binding. Named, default, and namespace imports are represented explicitly in the
linkage contract described below.

### Resolution is an exact filesystem policy

`kiln-module-resolver` accepts only explicit relative (`./`, `../`, `.`, `..`) and absolute
filesystem specifiers. It canonicalizes the project root, importer, and target; requires the
canonical target to remain inside the canonical project root; requires a regular file and one of
Kiln's supported source extensions; and preserves structured errors.

The resolver never guesses an extension, an `index` file, or a package entry point. Bare package
names, URLs, query/fragment specifiers, and package/Node/TypeScript resolution are not silently
interpreted. Those policies may be added above this boundary with a separate decision.

### Per-file module IR is a separate memoized query

The database keeps the existing parser-independent `ir(file)` query for one-file execution and
adds a separate memoized `module_ir(file)` query for linkage-aware bytecode. Both depend on the
file's parse result, so ordinary source and HIR memoization remains shared. `module_ir(file)` emits
one `kiln-ir::ModuleProgram` containing the ordinary program plus runtime dependency specifiers,
import bindings, and local export slots.

This separation keeps the existing single-file interpreter/native APIs stable while making module
linkage explicit. It also gives incremental callers a precise invalidation unit: editing one file
recomputes its parse and module IR, while graph traversal decides which entry-rooted modules need
to be visited.

### Graph traversal is one explicit DFS

`kiln-module-graph` owns entry-rooted traversal, resolution, module-IR linking, and dependency-first
order. It accepts either:

1. a source loader plus frontend, or
2. a caller-owned HIR provider.

The second form is the integration seam for the daemon and editor overlays. The provider can return
`engine.parse(path).module` from the existing memo, so graph construction does not bypass overlays
or pay for a second parse.

The builder performs one explicit depth-first traversal with `Visiting`/`Done` states and a path
stack. It resolves each runtime dependency once, visits it, and records a deterministic
dependencies-first order. It does not ask the query database to recursively compute another
module's `module_ir`; this avoids confusing a user module cycle with an internal memo-query cycle.

A back-edge returns `ModuleGraphError::Cycle` containing the closed canonical path sequence. The
first scalar slice reports cycles as structured unsupported input. It does not claim that a
topological order is ECMAScript-correct for cycles.

### Current executable linkage slice: acyclic imports, re-exports, and namespaces

`kiln-ir::compile_module` predeclares runtime named and default imports as compiler-assigned local
slots and emits `ImportBinding { source, imported, local, local_slot }`. Local named/default exports
resolve to `ExportBinding { exported, local_slot }`. The graph checks that each import source is
resolved, each imported name exists unambiguously in the provider's exports, and no module declares
the same explicit export name more than once.

The current interpreter initializes those import slots from dependency results, evaluates modules
in the recorded dependencies-first order, and publishes the declared export slots. Namespace
imports and namespace re-exports use an interpreter object whose keys are sorted and whose values
are a snapshot taken after the provider evaluates. Named/default indirect re-exports are lowered to
private import slots and then published under their visible names. Default expressions and
function/class/default declarations use ordinary compiler-owned export slots.

Star exports are resolved once at link time in dependencies-first order. Each visible name carries
the canonical origin of its binding rather than only a boolean "provided" bit. An explicit export
wins over the same name arriving through a star, `default` is never forwarded, and two star paths
to the same canonical binding remain one export. Two different bindings with the same name are
ambiguous: a named import receives `ModuleGraphError::AmbiguousExport`, while a namespace object
omits that name. Resolution is iterative over the already-acyclic graph, so valid deep barrel chains
have no arbitrary recursion-depth limit.

Cycles remain an explicit unsupported case as described above. Other unsupported syntax is
preserved in HIR and produces source-located compile/link errors; it is not silently treated as an
empty module. Type-only imports/exports do not create runtime slots or runtime edges. Side-effect
imports have no `ImportEntry`, so their non-type-only requested-module occurrence remains a runtime
dependency and is evaluated for effects.

### Live bindings remain incomplete at the namespace and cycle boundaries

Mutable module bindings use cells, so a named importer observes later writes made through the
exporting module. Namespace objects are still snapshots, and the graph still evaluates only an
acyclic dependency order. Full ECMAScript linkage requires namespace properties to read through
the provider bindings and module instantiation to allocate all bindings before SCC-based
evaluation, preserving TDZ reads and live updates across cycles.

### Native units follow the module boundary

The native backend emits one C translation unit per linked module. Module dependency order and
binding resolution stay in the graph/linker; the layered artifact cache stores generated C,
per-module objects, runtime/launcher objects, and the final link as separate artifacts. This keeps
an unchanged module object reusable when a sibling module or the runtime changes, and leaves room
for the future cell-based ABI without collapsing all modules into one generated source file.

## Consequences

- Oxc upgrades remain localized to `frontend-oxc`; downstream crates consume owned, stable linkage
  data rather than arena-borrowed parser records.
- The resolver is predictable and safe for the initial project model, but users must write explicit
  filenames and cannot use packages until a higher-level resolver policy exists.
- Parse and per-file module IR remain incrementally memoizable even though graph traversal is
  entry-rooted and explicit.
- Acyclic named/default/namespace imports, local exports, re-export aliases, default expressions,
  and star expansion have a usable interpreter path today. Missing and ambiguous named exports
  fail with structured diagnostics; ambiguous names are absent from namespace objects.
- Mutable named imports read through binding cells. Namespace snapshots and the lack of
  instantiate-then-evaluate SCC support remain explicit differences from full ECMAScript modules.
- Native compilation inherits one translation unit per module and the same graph/linkage boundary;
  artifact caching can therefore invalidate only affected module objects and link inputs.
