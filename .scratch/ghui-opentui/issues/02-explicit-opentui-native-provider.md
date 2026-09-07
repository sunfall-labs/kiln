# Give OpenTUI an explicit native provider

Status: needs-triage

## Failure

OpenTUI 0.4.2 rejects the honest Kiln platform identity before it can initialise its renderer.
Changing the identity only reaches the next absent contract: the package requires `node:ffi` or
`bun:ffi`, dynamic-library loading, pointers and array-buffer views, and same-thread callbacks.

## Decision needed

Choose whether Kiln should implement a reusable constrained `node:ffi` surface or a dedicated
OpenTUI provider linked into native applications. The generic surface is more reusable but is a
new capability and security boundary. A dedicated provider is narrower, but needs an integration
contract that does not fork or patch OpenTUI's JavaScript package.

The implementation must verify the selected OpenTUI library and its symbol table at packaging or
link time, as the native-number provider does, and must not advertise `process.platform` as a host
that Kiln does not implement.
