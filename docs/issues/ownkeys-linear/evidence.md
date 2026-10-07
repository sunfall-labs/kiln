# Proxy `ownKeys` target invariant snapshot

Measured from the owned `perf/luna-sweeps` worktree on 2026-09-27. The pre-change binary is the
immutable `e7591c17` baseline at `~/Developer/kiln-mass-opt/docs/issues/mass-opt/baseline-kiln`
(SHA-256 `9e0e75266635a045b63668400c5ef8d16174aef9289126728ffed0bcccc7a021`). The candidate is the
release build from `afc83a0a` plus this change (SHA-256
`9fcdf1bea00a58061426894a35e3ccdeb30ad4b232a0a377264604440c860423`). Both measurements used the
pinned Test262 checkout `771005236e88a909635104e03ba12559688c0172`, `KILN_JOBS=4`, and a 4096 MiB
process cap. Runs were alternating A/B/B/A; every variant passed.

| Binary | sloppy | strict | sum |
| --- | ---: | ---: | ---: |
| Before | 1.223 / 1.227 s | 1.212 / 1.209 s | 2.435 / 2.436 s |
| After | 0.698 / 0.695 s | 0.701 / 0.690 s | 1.399 / 1.385 s |

`Proxy/ownkeys-linear.js` creates a plain object with 7,500 non-configurable and 7,500 configurable
properties, then returns all 15,000 keys in reverse order from the proxy trap. The profile attributed
78 samples to `StringUnits::units`, 47 to `UnitIter::next`, and 43 to pointer addition. Source tracing
showed the quadratic operation: after trap-result processing, the invariant loop called `own_state`
for each held key, and ordinary object lookup linearly searched the target's property vector.

For a non-wrapper `Value::Object`, the invariant check now snapshots each slot's configurability
once, preserving the first-match behavior of the generic lookup, then checks keys in the existing
held-key order against the same trap-result `HashSet`. The snapshot is local to this invariant check;
arrays, string wrappers, proxies, and other exotic targets retain the generic operation path. It is
built only after the trap and all trap-result getters finish, so mutations made while materializing
the list affect the checked target state.

The entire pinned `staging/sm/Proxy` subtree selected 43 variants. Before and after, 26 passed and 17
failed with identical case IDs and outcomes; no existing failures changed. The regression probe
`proxy-ownkeys-membership` now also checks omission of a configurable key on an extensible target,
required reporting after `preventExtensions`, and a non-configurable key added by a trap-result
getter. `target/release/kiln test conformance/regress --node` passed all 18 probes. The interpreter
crate suite passed 197 tests, zero failed, one ignored. The parent worktree owns the full gate.
