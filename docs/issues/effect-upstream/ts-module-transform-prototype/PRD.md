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

Before enabling the full stage, design these changes explicitly:

- HIR module source representation: generated parser input plus optional
  original source and source map; make coordinate domain explicit.
- Parser diagnostics: map generated positions back through the TS transform
  map; retain original parser/transform diagnostics in original coordinates.
- Runtime errors and stack metadata: generated code executes, but source-facing
  locations should resolve to original TS where a map exists. Function source
  spans remain generated JS.
- Module/compile/native identity: include original source digest, transformed
  JS digest, transform implementation/version/options, and the source map (or a
  stable digest of it) whenever diagnostics depend on it. Cache identity must
  not conflate two transforms that emit the same JS but map diagnostics
  differently.
- Transform policy: document supported TS compiler options and the relationship
  to existing custom HIR erasure. Compare enums, namespaces, decorators, JSX,
  import elision, class fields, and early errors before replacing existing
  lowering semantics.

The current `Module.source` / `Program.source` contract remains intact if those
fields are redefined to mean the exact parser-input JS and a distinct original
source field is introduced. It is not intact if only closure source slices are
rewritten while HIR/IR spans continue to index the original TS.

## Decisions deferred

No default transform is enabled before the user chooses whether static TS
functions should reflect emitted JavaScript. No native/runtime source mapping,
module cache-key change, or broad upstream run is part of the first phase.
