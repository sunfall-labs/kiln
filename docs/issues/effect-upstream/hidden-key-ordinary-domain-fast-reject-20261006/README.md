# Early reject for ordinary strings in hidden runtime-key checks

`kiln_c99_get_key` checks whether every named key is one of six runtime-private spellings. All six exact names are reserved (`@@`) keys; ordinary program strings, including strings with identical code units, are not hidden. The helper now returns false immediately when the key is not in the reserved symbol/private domain, then retains the same exact-name loop for reserved keys. This removes repeated candidate-name checks from the ordinary read path without changing realm state or key publication.

The exact spellings and existing `kiln_c99_key_is_reserved` flag predicate were audited in the same source snapshot. The backend fixture checks all six spellings in private and symbol domains, verifies ordinary strings with those same spellings remain visible, and covers near-prefixes, null, UTF-16 and interned keys. The conformance probe checks literal/computed reads and own-key enumeration.

## Validation

Source base: `ab13f5ff398a2ff665cc8c3feccdb7437d0e26fb`; candidate source commit: `e57daedab6a2c6ed395a309ea1ee564868f82d60`.

- Pinned Rust toolchain: `rustc 1.97.1 (8bab26f4f 2026-07-14)`.
- Focused test `hidden_runtime_key_check_skips_only_ordinary_string_domain`: passed (also included in the full suite).
- Full `kiln-backend-c`: 349 passed, 0 failed, 4 ignored; emitted-C integration tests: 37 passed, 0 failed.
- Node oracle probe: completed with the expected six ordinary property values and six enumerable keys.
- `scripts/native.sh conformance/native/hidden-runtime-key-string-domain.js`: 1 matched, 0 refused, 0 diverged.
- Candidate debug CLI SHA-256: `24543ae792fe1e898adf63851cb26ea8466fb1951a8d54890fd0bdb410e9fd11`.
- Provider manifest SHA-256: `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`; archive SHA-256: `61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d`.

The prior profile attributed about 200 ms of sampled CPU to `kiln_c99_is_node_hidden_key_key` within an 8.70-second sampled window (878 samples). That is an investigation bound, not a measured speedup; this change has no benchmark claim.

## Excluded stale-snapshot attempt

The first full backend run used stale source base `02eeb049b79a217a34c499a23b49b6fe523179ba`. It reported 348 passed, 1 failed, 4 ignored because that snapshot's mutable-static inventory test still counted test-only store counters. The subsequent CLI build completed against that stale source and was not used for native evidence. Both logs are retained under `evidence/excluded-stale-base-02eeb049/`; neither result is a verdict for this candidate.

## Evidence files

`evidence/backend-current.log`, `cli-current-build.log`, `native-parity.log`, and `node.log` preserve the candidate run output. The excluded older attempt is kept in its separately labelled directory. SHA-256 values can be recomputed from the committed files.
