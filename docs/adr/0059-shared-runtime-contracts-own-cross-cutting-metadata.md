# ADR 0059: Shared runtime contracts own cross-cutting metadata and dispatch policy

## Status

Accepted.

## Decision

Cross-cutting runtime metadata has one authoritative owner per boundary:

- `kiln-native-provider-contract` owns the private number-provider schema, ABI version, format and
  parse versions, feature profiles, archive and manifest names, export registry, and ordered source
  inventory. Packaging and checker scripts consume its contract CLI instead of copying registry
  fields or source-digest algorithms into shell.
- The removed `kiln-node-abi` crate is not replaced by another parallel ABI crate. Node host
  operations use the typed `kiln-host-abi` boundary, while provider-specific exports remain in the
  provider contract and registry.
- Test262 checkout validation, pinning, physical subtree selection, corpus guards, memory-limited
  execution, and result-shape validation live in `scripts/test262_corpus.py`. The fetch, full-run,
  and chunk-baseline scripts are adapters; the baseline retains its separate per-chunk comparison
  and update policy.
- CommonJS resolution, lazy target execution, cache identity, cycles, and thrown-value transport
  are owned by the graph's CommonJS loader. Both interpreter and native graph execution enter that
  loader through the graph execution interfaces rather than maintaining parallel CommonJS
  dispatch or cache implementations.

## Consequences

Changes to one of these contracts update the owner and its consumers together. Shell entrypoints
remain responsible for command-line policy and diagnostics, while metadata, corpus safety, and
CommonJS runtime semantics are checked at their shared boundaries. Removing a legacy ABI or
dispatch path requires updating the owning typed boundary instead of introducing a compatibility
copy.
