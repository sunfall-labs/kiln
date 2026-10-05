# Static TypeScript to JavaScript source prototype

Status: isolated candidate routes static TypeScript through real Oxc transform/codegen; focused frontend, native module, and Formatter interpreter/native checks pass; full graph-suite acceptance remains pending

## Goal

Test whether a real per-module TypeScript transform can make reflected function
source equal the generated JavaScript, while preserving original TypeScript for
diagnostics. This is not a source-slice formatter and does not bundle modules.
The Formatter transform is a focused fixture, not the acceptance suite.

## Prototype boundary

The isolated candidate spans `kiln-frontend-oxc`, `kiln-hir`, IR source-location
and cache identity plumbing, and native DCE reparsing. Static TypeScript
entrypoints use pinned Oxc 0.147, a Node 22 target, TypeScript defaults,
automatic JSX, and Oxc codegen defaults. Each module is transformed, emitted
with source-map data, then reparsed from generated JavaScript as the actual HIR
input. Explicit-goal, dynamic-source and JavaScript paths remain unchanged. The
frontend suite passes 88/88; the focused static TypeScript `import.meta` native
DCE test passes; and both Formatter engine replays pass 67/67 against official
Node/Vitest, with zero source-transform gaps. Full 322-file acceptance remains
pending.

Focused prototype coverage:

- A typed, multiline arrow is erased, receives generated JS punctuation and
  indentation, and its reparsed function span slices the generated buffer.
- Source-map output identifies the original `.ts` path and embeds/maps back to
  original TypeScript coordinates.
- A representative enum, namespace, JSX/TSX, and type-only import source is
  transformed and reparsed without changing the existing frontend parser's HIR
  module/import/export meaning where the transform supports the syntax.
- Plain `.js`, eval, and Function-constructor parsing remain on their current
  path; those tests assert their current source-spelling behavior.

Earlier evidence: `static_typescript_codegen_is_parser_input_and_maps_back_to_ts`
and `static_typescript_codegen_keeps_module_runtime_syntax_and_erases_types`
passed when codegen was test-only. New policy-level cases cover actual frontend
entrypoints, CommonJS requires, generated-buffer spans, provenance offsets,
TSX/module edges, and identity. A long multiline coordinate-index test covers
CRLF, U+2028/U+2029, Unicode surrogate boundaries, and invalid columns. No
evidence yet establishes complete equivalence with the custom HIR TS path or Vite
for arbitrary modules.

## Full-stage model

The eventual stage would be called per resolved static source module, before
HIR lowering. It keeps the module graph and all module boundaries: transform a
single module, retain its import specifiers, then let the existing graph
resolver link those imports. No chunking or bundling occurs. JS inputs bypass
the transform. Dynamic `eval`, indirect eval, `Function` constructors and
ShadowRealm source remain the dynamic-source path and preserve their own input
text.

The parser-input source must be the generated JS. In HIR/IR, the source buffer
that `Span` indexes and the function span used by interpreter/native
`Function#toString` must therefore be the generated JS buffer. The original TS
buffer and source map need separate ownership for diagnostics and source
locations; the existing UTF-16 `SourceOrigin` is not a source-map container.

### Concrete frontend wiring

Put the transform before lowering at a static-source frontend boundary, and
make that boundary explicit instead of routing dynamic source through it. One
internal operation should accept resolved path, original text, parse goal, and
module mode, and produce the final JS AST/module record, emitted JS, transform
diagnostics, and parser-independent source-map metadata. Keep module syntax and
file boundaries intact; the existing graph resolver still resolves imports.

The common static entrypoints are `parse` and `parse_dce` /
`parse_dce_native` for project ESM, plus `parse_commonjs` and
`parse_commonjs_with_requires` for CommonJS. The candidate transforms TS/TSX before DCE so
type-only imports are gone before dependency demand is computed. Derive CJS
`require` and other loader requests from the final AST and semantic scope so
type-only syntax cannot become a runtime edge. Static loader discovery must
consume the same transformed AST. Do not apply this to `parse_eval`,
`parse_eval_units`, eval, `Function` construction, or `parse_with_goal` used for
dynamic/conformance source. Keep `.js` and JSX-only paths unchanged. The
distinction should be explicit at the caller rather than inferred in a shared
dynamic-source helper from a `.ts` suffix.

Oxc configuration must remain explicit and versioned. Formatter's observed
Vite output used target `node22`; the current transform fingerprint records
`oxc=0.147.0;target=node22;typescript=default;jsx=automatic;codegen=0.147.0-default`.
This is an initial pinned policy, not a claim to match all Vite project config.
Before default adoption, JSX import source, enum/namespace and import-elision
policy, decorators/class-field assumptions, and sourcemap options should become
explicit config fields rather than relying on Oxc defaults.

### Source ownership and coordinate domains

Keep `Module.source` and `Program.source` as the exact parser-input buffer. For
static TS that becomes emitted JS, and all HIR/IR spans and `FunctionProto.span`
index that same buffer. Both engines then reflect actual compiled source from
the original function span, without a second printer in `Function#toString`.

The parser-independent `StaticSourceProvenance` now lives in `kiln-hir` with
original TS path/text, transform fingerprint, content identity digest, and compact
generated-to-original byte offsets. Oxc types do not cross that boundary. The
content identity hashes original source bytes, source kind/goal and transform
configuration/version, plus generated-to-original offsets; the absolute diagnostic
path is retained separately and does not fragment reusable generated artifacts.
The full map and original TS are shared from HIR into `Program` only for modules that
need later native-DCE or `import.meta` reparsing; prepared reparse consumes generated JS directly,
so it does not transform twice and the original map remains available. The separate
`static_source_identity` is hashed independently. `SourceOrigin` remains exclusively
the dynamic UTF-16 transport mapping. IR maps
compiler diagnostics and debug call-site line/column coordinates to TS. The
parser also remaps generated-parser diagnostics. Imported-module
codeframe rendering still needs focused CLI verification before this is
complete.

Relevant handoffs are `kiln_hir::Module` in
`crates/syntax/kiln-hir/src/lib.rs`, `kiln_ir::Program` and
`FunctionProto.span` in `crates/compile/kiln-ir/src/lib.rs`, `Compiler::new`
and debug-coordinate materialization in `crates/compile/kiln-ir/src/compile/`,
and `finish_run_summary` / `render::diagnostic` in `crates/tools/kiln-cli/src/`.
The frontend must preserve original source for diagnostic rendering; the
original-source byte offsets and generated parser spans are separate domains.

### Cache identity

The memo database currently hashes the original file text (`Parsed::new`),
while native program identity hashes bytecode, `Program.source`, source-origin
transport, separate static transform identity, function metadata, and debug call sites
(`kiln-backend-c/src/identity.rs`). Keep raw TS as the frontend/query input so
editing source invalidates parsing even when output JS is unchanged. Include
transform fingerprint and source-map identity in HIR/provenance equality for
red-green queries, and in persistent module identity whenever map data can
change diagnostics or original debug locations. Mapped debug call sites remain
covered by the existing program hash. This prevents stale generated code or a
different TS mapping from being served under the same artifact key. The static
transform identity excludes absolute source paths: diagnostics retain those paths,
while identical code from another checkout can reuse the same object artifact.

The candidate frontend routes and program/native identity plumbing have focused
coverage, but remain isolated from the root milestone. Before landing, compare enums,
namespaces, decorators, JSX, import elision, class fields, early errors, and
CommonJS discovery with current lowering, then run both engine graph suites.
No change is intended for JS, dynamic-source, or module-boundary semantics.

Before enabling the full stage, acceptance must cover:

- Parser, transform, and compile diagnostics for entry and imported modules
  render codeframes from original TS, while all executable HIR/IR spans index
  generated JS.
- Stack call sites point to original TS coordinates when a map exists;
  `Function#toString` slices emitted JS in both interpreter and native.
- TypeScript enum/namespace, decorators, JSX, type-only import elision,
  class-field policy, CommonJS loader discovery, and early errors retain
  specified runtime/linking behavior.
- JS, eval, `Function`, explicit Script/Module conformance inputs, and module
  graph boundaries remain unchanged.
- A changed input, transform config/version, generated output, or source map
  cannot hit a stale query/native artifact, and full interpreter/native graph
  tests pass before default adoption.

The current `Module.source` / `Program.source` contract remains intact if those
fields are redefined to mean the exact parser-input JS and a distinct original
source field is introduced. It is not intact if only closure source slices are
rewritten while HIR/IR spans continue to index the original TS.

## Decisions deferred

The static TS reflection preference remains open pending the user's choice about
reflecting compiled JavaScript for static TypeScript. This candidate lives in an
isolated worktree and does not change the root checkout's current user-visible behavior.


## CLI discovery boundary

Node dependency synchronization currently has two owners. The CLI's
`include_reachable_dependencies_with_extra_includes` parses each reachable
source to derive runtime import/require requests before source synchronization.
The daemon then receives original source text in `OwnedGraphSnapshot` and parses
it again to construct the executable graph. The snapshot transports source text
and resolution metadata, not prepared HIR/provenance, so this boundary currently
repeats static TS preparation across the CLI/daemon handoff. Same-process native
DCE and `import.meta` reparses reuse the shared provenance and generated buffer.
Removing cross-boundary work requires an explicit prepared-module payload in the
owned snapshot/request model; a process-global cache is intentionally not used.
Discovery must continue observing post-transform runtime imports, so parsing
authored TS without type-only import erasure is not equivalent.
