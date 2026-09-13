# ADR 0059: Shared runtime contracts own cross-cutting metadata and dispatch policy

## Status

Accepted.

## Decision

Cross-cutting runtime metadata has one authoritative owner per boundary:

- `kiln-native-provider-contract` owns the provider metadata view consumed by packaging and checker
  scripts. Its ABI/export values are derived from the existing `kiln-native-number-abi` registry,
  its format and parse values come from `kiln-native-number`, and its ordered source inventory is
  the existing `crates/kiln-native/provider-sources.txt`. The contract CLI is the shell boundary;
  source digest computation and manifest metadata are not reimplemented in shell.
- Removing `kiln-node-abi` does not move that API into a host-operation ABI. The replacement is the
  TypeScript builtin descriptor/export path used by the builtin registry, which keeps Node builtin
  surface metadata with the builtin sources.
- Test262 checkout validation, pinning, physical subtree selection, corpus guards, memory-limited
  execution, and result-shape validation live in `scripts/test262_corpus.py`. The fetch, full-run,
  and chunk-baseline scripts are adapters; the baseline retains its separate per-chunk comparison
  and update policy.
- CommonJS callable dispatch uses the existing interpreter `call_value` path through `invoke_dispatch`
  for its two supported callable variants. This keeps receiver/realm entry behavior in the normal
  call machinery without adding a second direct-call implementation.

## Consequences

Changes to one of these contracts update the owner and its consumers together. Shell entrypoints
remain responsible for command-line policy and diagnostics, while provider metadata, corpus safety,
builtin descriptors, and callable dispatch each have one implementation boundary. Removing a
legacy ABI or dispatch path requires updating that owner instead of introducing a compatibility
copy.
