# Empty Map/Set `forEach` with deleted history

`Map` and `Set` retain deleted entries as tombstones while `count` preserves iterator
semantics. Calling `forEach` after every entry has been deleted used to scan that entire
history despite having no callbacks or mutations to observe. The native fast path now checks
`live == 0` after validating that the callback is callable and returns `undefined`; the
interpreter performs the same validation before its unchanged mutation-visible loop.

The regression probe covers empty and fully deleted collections, non-callable callbacks,
construct-only classes, revoked callable proxies, zero callback invocations, and insertion
during a nonempty iteration. The native backend C fixture covers the empty/deleted callback
validation and valid-but-uninvoked callback behavior. Validation passed:

- `mbx test -p kiln-backend-c --lib --offline collection_for_each_validates_callback_before_empty_history_fast_return -- --test-threads=1`: 1 passed.
- `scripts/native.sh conformance/native/collections-empty-foreach.js`: 1 matched, 0 refused, 0 diverged (Node and interpreter output also matched).
- `cargo fmt --check` and `git diff --check` passed.

## Bounded workload

`bench/non-io/collection-forEach-deleted-history.mjs` builds 50,000-entry Map and Set values,
deletes every entry, then measures repeated no-op `forEach` calls. The checksum is `0` for every
run. The old implementation measures about `0.02685 ms` per call for both types. The candidate
measures about `0.000107 ms` per call, roughly 250x faster for this deliberately tombstone-heavy
case. Node and Bun also returned checksum `0`, but this harness's millisecond clock rounded their
per-call values to `0`.

The baseline AOT executable was built by CLI SHA-256
`e7a06733713ec8acfb7e3cc981ad78b1d881befa57b6d4ab455c3ed33f5139a7` at
`~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/frozen-clock-native/kiln`;
its AOT output SHA-256 is `1b29daa0a83aded4eac8eb270e78ff43fa1ba87941f263c26c54154e7c3e93ce`.
The candidate AOT executable SHA-256 is
`5462afcf063ee73ad35e8ea43e17b570f638041cd8173d45c21a336b8fb847b4`. Candidate build reported
188.8 seconds total: 10.9 seconds emission, 177.2 seconds compilation, and 0.12 seconds linking;
generated C was 52.1 MB. This is a focused synthetic case, not evidence of an improvement in
the Sharding timeout workload.
