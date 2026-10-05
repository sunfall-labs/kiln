# Static TypeScript to JavaScript source prototype

Status: frontend prototype passes, default behavior unchanged

## Goal

Test whether a real per-module TypeScript transform can make reflected function
source equal the generated JavaScript, while preserving original TypeScript for
diagnostics. This is not a source-slice formatter and does not bundle modules.
The Formatter transform is a focused fixture, not the acceptance suite.

## Prototype boundary

The first phase lives only in `crates/syntax/kiln-frontend-oxc`: add a test-only
Oxc 0.147 TS-transform/codegen helper that accepts one source module, produces
JavaScript plus an Oxc source map, and reparses the generated JavaScript as a
JavaScript module. It does not replace `parse`, affect JS or dynamic source, or
change any runtime default. The helper should use the existing Oxc parser,
`Transformer`, and `resolve_transform_options` policy, then Oxc codegen with
source-map generation. It should not use a Vite-only indent override; defaults
must naturally produce the Formatter body shape if that match is available.

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

Initial implementation evidence: `static_typescript_codegen_is_parser_input_and_maps_back_to_ts`
and `static_typescript_codegen_keeps_module_runtime_syntax_and_erases_types`
pass in `kiln-frontend-oxc` (2 passed). They use only a test dependency on the
pinned Oxc 0.147 code generator; production parsing and runtime paths are
unchanged. The TSX fixture covers JSX, a runtime enum/namespace, retained
runtime import and erased type-only import. This is a capability prototype,
not yet evidence that replacing the custom HIR erasure path preserves all
TypeScript semantics or matches Vite for arbitrary modules.

## Full-stage model if the user selects it

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
`parse_commonjs_with_requires` for CommonJS. Transform TS/TSX before DCE so
type-only imports are gone before dependency demand is computed. Derive CJS
`require` and other loader requests from the final AST and semantic scope so
type-only syntax cannot become a runtime edge. Static loader discovery must
consume the same transformed AST. Do not apply this to `parse_eval`,
`parse_eval_units`, eval, `Function` construction, or `parse_with_goal` used for
dynamic/conformance source. Keep `.js` and JSX-only paths unchanged. The
distinction should be explicit at the caller rather than inferred in a shared
dynamic-source helper from a `.ts` suffix.

Oxc configuration must be explicit and versioned. Formatter's observed Vite
output used target `node22`; `TransformOptions::default()` is not proof of that
policy. Define a stable config covering target, JSX runtime/import source,
TypeScript enum/namespace and import-elision policy, decorators/class-field
assumptions, and sourcemap policy. Include its version/fingerprint in
compilation identity. The current prototype intentionally uses defaults only
to verify the transform/codegen/reparse seam; it must not define production
target policy.

### Source ownership and coordinate domains

Keep `Module.source` and `Program.source` as the exact parser-input buffer. For
static TS that becomes emitted JS, and all HIR/IR spans and `FunctionProto.span`
index that same buffer. Both engines then reflect actual compiled source from
the original function span, without a second printer in `Function#toString`.

Add a parser-independent provenance type in `kiln-hir` alongside `source`:
original TS text/path, transform fingerprint, and compact generated-to-original
mapping segments. Do not pass Oxc AST or `SourceMap` types across this crate
boundary. Keep this distinct from `SourceOrigin`, which currently maps a
parser-safe UTF-8 transport back to dynamic UTF-16 source units. Propagate
provenance into IR long enough to map compile diagnostics and debug call-site
coordinates. The native runtime can receive original path/line/column in its
existing debug-call-site table; function source still uses emitted JS. Map
post-codegen parser diagnostics back to original TS before rendering a
codeframe, since a generated-JS span cannot be applied to original TS text.

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
transport, function metadata, and debug call sites
(`kiln-backend-c/src/identity.rs`). Keep raw TS as the frontend/query input so
editing source invalidates parsing even when output JS is unchanged. Include
transform fingerprint and source-map identity in HIR/provenance equality for
red-green queries, and in persistent module identity whenever map data can
change diagnostics or original debug locations. Mapped debug call sites remain
covered by the existing program hash. This prevents stale generated code or a
different TS mapping from being served under the same artifact key.

Stage implementation in this order: add the parser-independent provenance
type and frontend transform helper; wire static entrypoints and generated-JS
spans; map parser/compiler diagnostics and stack sites; then include
provenance identity in query and native artifact caches. Before changing the
default, compare enums, namespaces, decorators, JSX, import elision,
class-fields, and early-error behavior with current lowering, then run both
engine graph suites. No change is needed to JS, dynamic-source, or module
boundary semantics.

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

No default transform is enabled before the user chooses whether static TS
functions should reflect emitted JavaScript. No native/runtime source mapping,
module cache-key change, or broad upstream run is part of the first phase.
