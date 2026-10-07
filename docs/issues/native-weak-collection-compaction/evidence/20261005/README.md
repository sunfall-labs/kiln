# Native weak-collection compaction evidence

Source commit: `d728f13e103dd918fe925d467b039ebe25d4d4b1` plus the local test-only follow-up
that makes the churn fixture honor `KILN_GC_STRESS`. No CLI was built and no upstream Hash corpus
was rerun for this evidence.

The C fixture normally disables automatic collection while it inspects weak entry storage. When
`KILN_GC_STRESS` is truthy, it now turns on the initialized heap's stress mode only after creating
and rooting both sets, the churn key, and the keeper key. It performs a full mid-turn collection
every 31 operations during the 10,000 insert/delete cycles, then disables stress before the
explicit dead-key ephemeron checks. This isolates collection/barrier safety during compaction while
keeping the dead-key assertions deterministic.

Both invocations used `mbx test -p kiln-backend-c --offline --jobs=1
weak_collection_churn_compacts_history_and_preserves_ephemerons -- --nocapture
--test-threads=1`, with `CARGO_BUILD_JOBS=1`, `KILN_JOBS=1`, `RAYON_NUM_THREADS=1`, and
`MBX_GC_AUTO=false`.

- Stress run: `KILN_GC_STRESS=1`; session 11051 completed successfully, 1 passed, 0 failed; test
  body 1.60s.
- Ordinary run: `KILN_GC_STRESS` unset; session 28554 completed successfully, 1 passed, 0 failed;
  test body 1.62s.
- The pre-existing `weak_collections_mark_values_to_a_fixed_point_and_clear_dead_keys` and
  `collection_for_each_validates_callback_before_empty_history_fast_return` focused tests passed
  before the stress-only fixture change. The compactor implementation itself was unchanged by
  that follow-up.

`KILN_GC_STRESS=0` remains disabled in the fixture, matching heap initialization semantics. The
focused stress mode exercises deletion compaction with collections interleaved; the final
dead-key/fixed-point assertions use explicit deterministic collections. These are backend C
fixtures only, not native CLI or corpus evidence.
