# Native provider contract

Layer 1 (`semantics`). Owns the versioned provider ABI registry, capability and package metadata,
source manifest, and vendored native number conversion sources.

`abi` declares signatures and ownership; `number` exposes the C number runtime. Neither contains
the provider's realm or host implementation. Those implementations remain in
`kiln-native-provider-ffi` in layer 4b, while allocation-free Rust numeric algorithms remain in
`kiln-number-core` in layer 0.

Run `mbx test -p kiln-native-provider-contract`. Provider packaging also verifies archive exports
against the registry.
