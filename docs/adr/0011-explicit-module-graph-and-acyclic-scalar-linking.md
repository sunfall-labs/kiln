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

The initial execution surface is deliberately smaller than the module grammar. The scalar
interpreter has no object namespace value, and the first linker evaluates dependencies before their
importers. Cycles therefore need a user-facing structured result while the representation is still
being prepared for ECMAScript's instantiate-then-evaluate semantics.

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
claims a runtime binding. Named and default imports are represented in the linkage contract even
though only the scalar forms described below are executable today.

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

### First executable linkage slice: scalar named/default imports and local exports

`kiln-ir::compile_module` predeclares runtime named and default imports as compiler-assigned local
slots and emits `ImportBinding { source, imported, local, local_slot }`. Local named/default exports
resolve to `ExportBinding { exported, local_slot }`. The graph checks that each import source is
resolved, each imported name exists in the provider's exports, and no provider exports the same
name more than once.

The current interpreter initializes those import slots from dependency results, evaluates modules
in the recorded dependencies-first order, and publishes the declared export slots. This is correct
for the acyclic scalar subset and makes missing sources/exports deterministic diagnostics.

The following forms remain explicit unsupported cases in this first linker:

- namespace imports (`import * as ns`) because scalar `Value` has no namespace/object form;
- star exports (`export *`) because their names and ambiguity rules require graph-level expansion;
- namespace re-exports (`export * as ns`) for the same object reason;
- indirect/named re-exports (`export { x as y } from ...`) because they need provider aliases;
- default-expression exports, which require a compiler-owned binding slot rather than a discarded
  expression;
- function/class/default declaration exports until functions and objects are in the executable
  HIR;
- cycles, as described above.

Unsupported forms are preserved in HIR and produce source-located compile/link errors; they are not
silently treated as empty modules. Type-only imports/exports do not create runtime slots or runtime
edges. Side-effect imports have no `ImportEntry`, so their non-type-only requested-module
occurrence remains a runtime dependency and is evaluated for effects.

### Live bindings are the next representation step

The first acyclic implementation copies dependency values into import slots. That is an intentional
subset boundary, not the final ECMAScript binding model. The next linkage representation will use
binding cells (or equivalent indirections): local exports point at cells, and every imported read
dereferences the provider cell rather than taking a snapshot. Module instantiation will allocate
all cells before evaluation; SCC-based evaluation will then make cycles possible while preserving
TDZ reads and live updates. Re-export aliases and star expansion belong on that same cell-based
linker rather than being retrofitted into copied values.

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
- Acyclic named/default scalar imports and local exports have a usable interpreter path today;
  cycles, namespace values, star expansion, re-export aliases, and default expressions fail
  explicitly with structured diagnostics.
- The current copy-based import slots do not yet provide live bindings. The cell/SCC design is
  recorded here so that future support extends the linkage model instead of changing the meaning
  of existing HIR fields.
- Native compilation inherits one translation unit per module and the same graph/linkage boundary;
  artifact caching can therefore invalidate only affected module objects and link inputs.
