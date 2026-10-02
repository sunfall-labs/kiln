# CommonJS context authority in direct graph transfer

Status: implemented; focused runtime, launcher and public verification passed, 2026-09-29.

The compiler-derived inventory found `KILN_GC_COMMONJS_REQUIRE` missing from the incoming heap
walk. `probes/actors-direct-commonjs-context.mjs` reproduces the public consequence: the
interpreter moves an unaliased plain graph after `createRequire`, while native strict audit
reports `ledger clear, heap unproven` and refuses the transfer. The factory does not capture or
publish its result. The existing full scan safely refuses this unclassified kind.

The require object contains executable code and an opaque context. The ordinary launcher
passes its CommonJS graph context; `createRequire` passes a per-importer context referring to
the graph. Shared graph launchers have corresponding contexts. Their import/export/module
values already have generated root visitors used by collection and incoming-root checks.
Pointer-free allocation and GC's existing no-trace case do not prove that the opaque context
contains no sender reference.

## Complete contract

Reuse each launcher's existing typed root visitor for its require context. Extend the internal
require-construction contract to carry an explicit context visitor and its root anchor alongside
the call function and call context. A generated unit wrapper must forward those authorities
from its launcher; it must not reconstruct a second list of module roots. Default native
`require` with a null context has no hidden context edges. A legacy non-null context without a
visitor remains unknown.

The incoming kind walk checks the direct context and anchor pointers against candidate storage,
then validates that the exact visitor and anchor are registered with the active heap. A missing
or expired registration remains unknown. The provider's independent root pass invokes each
registered inventory once, rather than once per require object. Retained exports therefore still
prevent transfer. Executable code pointers remain distinct from data pointers.

Ordinary and shared graph launchers register their stable root contexts before constructing require
objects. Their anchors outlive the callable contexts. Legacy opaque contexts without that authority
remain unknown. The registration query neither invokes callbacks nor marks values.

Collection and transfer consume one module-root inventory: collection is an adapter over the
read-only visitor. Adding a module root therefore changes the enumeration once. This avoids
separate field lists and repeated walks without declaring opaque pointers unconditionally safe.

## Required evidence

- A kind-level fixture checks a null/default context, an opaque unvisited context, a visited
  context retaining a candidate, and clear/retry after releasing the reference.
- The public CommonJS-sender probe succeeds in interpreter and native strict audit.
- A module export or require-context root retaining the candidate still prevents transfer;
  clearing it permits retry.
- Ordinary and shared graph initialization and teardown keep their root visitor contracts.
- The existing direct, prepared and CommonJS/Node probes remain correct under the appropriate
  collector and concurrent-realm checks. Keep source inventory and production admission claims
  separate until the rest of issue 10's store coverage is complete.
