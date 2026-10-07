# ADR 0076: Shared runtime contracts own cross-cutting metadata and dispatch policy

## Status

Accepted.

Filed as ADR 0059 on 2026-09-13, the same day as "Intrinsics are ordinary realm objects built from
one description", which kept that number. Renumbered 2026-10-02 so each number names one decision;
every existing reference to ADR 0059 meant the intrinsics ADR. The decision itself is unchanged.

## Decision

Cross-cutting runtime metadata has one authoritative owner per boundary:

- `kiln-native-provider-contract` owns the provider metadata view consumed by packaging and checker
  scripts. Its ABI/export values are derived from the existing `kiln-native-number-abi` registry,
  its format and parse values come from `kiln-native-number`, and its ordered source inventory is
  the existing `crates/native/kiln-native/provider-sources.txt`. `ProviderManifest` is constructed from
  named build facts and owns the canonical structured identity: it includes the provider policy,
  source/archive digests, and deployment facts, while deliberately excluding the relocatable
  archive filename. The contract CLI is the shell boundary; source digest computation and
  manifest metadata are not reimplemented in shell.
- Removing `kiln-node-abi` does not move that API into a host-operation ABI. The replacement is the
  TypeScript builtin descriptor/export path used by the builtin registry, which keeps Node builtin
  surface metadata with the builtin sources.
- Test262 checkout validation, pinning, physical subtree selection, corpus guards, memory-limited
  execution, and result-shape validation live in `scripts/test262_corpus.py`. The fetch, full-run,
  and chunk-baseline scripts are adapters; the baseline retains its separate per-chunk comparison
  and update policy.
- CommonJS callable dispatch uses the existing interpreter `call_value` path through `invoke_dispatch`
  for its two supported callable variants. `forwards_to_call_value` is the exhaustive classifier
  for value representations that forward to that path; `invoke_dispatch` retains the specialized
  typed-array, proxy, mutation, and native-call branches before the common tail. This keeps
  receiver/realm entry behavior in the normal call machinery without adding a second direct-call
  implementation.
- Gate result ownership is split between orchestration and evidence: `scripts/gate.sh` runs every
  step, while `scripts/gate_results.py` validates step result envelopes and writes the aggregate
  verdict. A gate run records `target/gate-logs/run.<id>/aggregate.json`, each step's envelope and
  raw log, and any step-specific result document. The result adapter reports the evidence it can
  parse; it does not turn every cargo stream into a machine-readable event protocol.

## Consequences

Changes to one of these contracts update the owner and its consumers together. Shell entrypoints
remain responsible for command-line policy and diagnostics, while provider metadata, corpus safety,
builtin descriptors, and callable dispatch each have one implementation boundary. Removing a
legacy ABI or dispatch path requires updating that owner instead of introducing a compatibility
copy.
