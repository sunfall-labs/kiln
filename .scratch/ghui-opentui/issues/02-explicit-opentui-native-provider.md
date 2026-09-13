# Give OpenTUI an explicit native provider

Status: in-progress

## Failure

OpenTUI 0.4.2 rejects the honest Kiln platform identity before it can initialise its renderer.
Changing the identity only reaches the next absent contract: the package requires `node:ffi` or
`bun:ffi`, dynamic-library loading, pointers and array-buffer views, and same-thread callbacks.

## Decision

Implement the reusable `node:ffi` surface. A dedicated OpenTUI-only provider would make this one
application run while leaving the package's public runtime contract unimplemented. The generic
surface is an explicit host capability: dynamic libraries, symbols, calls, pointers, array-buffer
views, and same-thread callbacks are owned by the Rust provider; JavaScript validation and the
Node-compatible facade are TypeScript built-ins over stable intrinsics.

The implementation must verify the selected OpenTUI library and its symbol table at packaging or
link time, as the native-number provider does, and must not advertise `process.platform` as a host
that Kiln does not implement.

## Implementation status

The provider ABI owns dynamic-library handles, prepared libffi call interfaces, typed calls,
same-thread callbacks, and precise error retrieval. Focused tests exercise a real `strlen` call,
stale-handle rejection after close, mixed integer/floating-point calls, callback round trips, and
active-callback unregistration refusal. The packaged static archive includes the ABI group in its
export and source-identity checks. The generated C bridge, TypeScript facade, borrowed views, and
callback rooting have mock coverage; the live ghui adapter remains under verification.
