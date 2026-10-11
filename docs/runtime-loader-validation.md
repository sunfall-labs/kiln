# Runtime loader

How the runtime loader treats module requests made at run time (`require`, `createRequire`,
dynamic `import()`). The loader's behavior is exercised by `conformance/regress` and the
`kiln-module-graph` and `kiln-conformance` crate tests.

Literal Node loader requests retain closed-world provenance: owner module, captured importer path,
and literal specifier. Computed or unsupported requests remain catchable runtime failures. A
`createRequire` closure captures its importer, normalizes a file URL to its `href` before the host
boundary, and resolves CommonJS targets through canonical graph cache cells.

Demand-only TypeScript builtins use the canonical graph value path and recursively materialize
retained dependencies. CommonJS cycles expose partially initialized shared exports. Thrown values
and loader failures remain realm-local. Failure lookup is owner-scoped and keyed by captured
importer plus literal specifier.

Dynamic import `type:"file"` validation is performed before graph plan reuse and rejects a source
module with the expected `TypeError`; only `ModuleSource::Asset` is accepted.

## Production and conformance loaders

The conformance loader keeps no ambient `BASE` state. Each capability now owns
an optional fixture base for importer-less eval/realm/agent requests; a request's explicit importer
always takes precedence. Two loader instances can therefore run interleaved without sharing a
resolver directory, while nested imports still resolve from their captured importer.

Production's immutable graph loader and conformance's fresh filesystem loader retain their
different adapters: both call `ModuleGraph::import_plan_for_request` for the shared
asset/import-attribute policy, while conformance preserves its `NotFound`/`Syntax`/`Unsupported`
classification and production preserves graph-owned deferred failures and cache identity.
