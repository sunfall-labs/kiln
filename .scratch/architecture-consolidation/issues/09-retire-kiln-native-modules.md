# 09. Retire `process.__kilnNativeModules`

Status: ready-for-agent
Blocked by: 08

Eight files in `crates/kiln-builtins/src` read host functions by string from
`process.__kilnNativeModules`, which both engines install (`kiln-interpreter/src/lib.rs` near
line 27489, `kiln-native/src/lib.rs` near line 6122). User code can see it, and a missing entry
turns into a function that throws at the call site.

Move each use to a declared host primitive (ADR 0065), then remove the property from both engines.

## Acceptance

- `grep -r __kilnNativeModules crates` finds nothing.
- `Object.getOwnPropertyNames(process)` no longer lists it in either engine, and matches node for
  that name.
- `kiln test conformance/node --node`, `kiln test conformance/server --node` and
  `scripts/native.sh` pass.
