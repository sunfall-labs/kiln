# Final validation record

This is an evidence record for the ghui native acceptance work. It does not claim that the
repository gate is complete.

## Gate evidence

The full gate run is `run.ZE857NrR`. Workspace validation completed with 77 suites, 1,607 passed,
and 10 failed. The failures mapped as follows:

- four `native_non_callable_*` diagnostics failures, fixed by formatting native provenance at the
  final uncaught graph boundary; the focused four tests pass, including `KILN_GC_STRESS=1`;
- one daemon cycle failure;
- one shared wrapper failure;
- one provider fixture failure;
- one namespace assertion failure;
- one Node ABI set failure;
- one HTTP failure.

The original Test262 comparison had three reported regressions. The restored baseline counts are:

- Async: `28/48`;
- statements: `17635/367` versus `17633/369` in the earlier report;
- expressions: `20578/392` versus `20507/463` in the earlier report.

The source phase unsupported set accounts for four cases and is covered by commit `f3cd2fa2`. The
last staging top-level-await result is `0/1`, matching the expected baseline.

## Focused native boundary validation

The four diagnostic tests pass in the owned worktree, both normally and with `KILN_GC_STRESS=1`.
Dynamic rejection identity coverage also passes:

- `native_dynamic_tla_rejection_preserves_original_error`;
- `native_dynamic_import_static_tla_target_rejection_is_cached`.

The integrated implementation commit is `156f525a`; the emitted C ABI snapshot follow-up is
`d870e2c5`. Commit `34453b92` is the namespace and ABI fix referenced by the gate mapping.

## ghui live acceptance

The latest live read-only run used compiler revision `fea2802b`, binary SHA-256
`c4ca4c0b36d2f24d533fba8119ac25ce1e56a13a886223246890c6c687cdbe0e`, and passed repository/PR/issue
navigation, PR #3 details, markdown scrolling, Escape back, `q` terminal restoration, and exit 0.
The earlier 15-second footer timeout remains bounded historical evidence; the latest transcript had
no timeout notice. No GitHub mutation was performed.

## Pending checks

The fresh same-host performance pairing against the main revision was not run, so no performance
baseline conclusion is recorded. Native stress remains active, and HTTP validation is pending.
