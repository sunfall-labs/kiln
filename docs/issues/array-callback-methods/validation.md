# Shared Array callback methods validation

This receipt records the candidate in the private `effect-array-sandbox` clone. The clone's base was `09cb9b75afa5b613fb105f60aaacabbc479ffdc5`; all source changes, including the additional prototype-chain and Date receiver fixes, are in the commit that adds this receipt. The pinned Test262 checkout is `771005236e88a909635104e03ba12559688c0172`. Test262 used the repository runner with its normal per-case limits, a 2 GiB memory cap, one worker, and no source or deadline overrides. Comparisons use frozen CLI `d0830baef65b01c9ccfeb626984e44cf863418a7008596188feb29822949ea88`.

The final native release CLI SHA-256 is `ecf0e5be832c12c59e7bf0aa346e1f67c76069ad1f2f5d57d1a4b152d7fb7a7a`. It was built with pinned Rust 1.97.1 / mbx 1.22.0 and `MBX_GC_AUTO=false`, `CARGO_BUILD_JOBS=1`, `MBX_SCHEDULER_CPUS=3`, `KILN_JOBS=1`, and `RAYON_NUM_THREADS=1`. Build log: `/tmp/kiln-array-final5-build.log`.

`cargo fmt --all -- --check` and `git diff --check` passed. The interpreter crate passed 262 tests, with 0 failures and 1 ignored. The native fixture `conformance/native/array-callback-bootstrap.js` matched the interpreter: 1 matched, 0 refused, 0 diverged. The same fixture under `KILN_NATIVE_GC_STRESS=1`, with only the million-hole case scaled from 1,000,000 to 1,000 slots, matched: 1 matched, 0 refused, 0 diverged, 0 stress probes skipped. Native realms for the fixture at N=1, 2, and 8 matched with no divergence.

| Test262 subtree | Candidate | Frozen baseline | Difference |
| --- | ---: | ---: | --- |
| `built-ins/Array/prototype/map` | 427/429 pass | 421/429 pass | Same 2 failures; 6 baseline failures removed |
| `built-ins/Array/prototype/reduce` | 509/517 pass | 507/517 pass | Same 8 failures; 2 baseline failures removed |
| `built-ins/Array/prototype/reduceRight` | 509/517 pass | 505/517 pass | Same 8 failures; 4 baseline failures removed |
| `built-ins/Object/defineProperty` | 2205/2250 pass | 2205/2250 pass | Exact same failing IDs |
| `built-ins/Object/defineProperties` | 1250/1264 pass | 1250/1264 pass | Exact same failing IDs |

The map failures are `create-species-abrupt.js` in sloppy and strict mode. The reduce failures are the `15.4.4.21-10-{3,4,6,7}.js` sloppy/strict variants; reduceRight has the corresponding `15.4.4.22-10-{3,4,6,7}.js` failures. Both Object descriptor subtrees have exactly the baseline failure sets. Every selected case was present and executed; no unsupported or skipped cases were reported.

The native fixture includes sparse/inherited indices, inherited proxy Get receiver and trap ordering, custom/null prototype links, callback mutation and right-fold truncation, Date setter receiver behavior for own/custom/function/proxy/null prototype cases, species behavior, and descriptor conversion/proxy snapshot/invariant controls. The stress copy is generated under `.scratch` and is intentionally not committed.

## Cross-realm cache follow-up

Read-only review found that the sparse-hole guard in `Intrinsics` was one `Cell<bool>`, even though intrinsic prototype records are keyed by realm. `record_prototype` recalculated that shared bit from whichever realm was most recently changed, so restoring realm B's default chain could disable the fallback required by a customized realm A. The guard is now a realm-indexed `Vec<bool>` (`crates/interp/kiln-interpreter/src/properties.rs`, `Intrinsics` and `record_prototype`); the callback hot path reads the relevant slot in `crates/interp/kiln-interpreter/src/object_helpers.rs::next_array_present_index`. In this representation an array without an explicit prototype link uses the primary realm; arrays with a foreign intrinsic link already decline the shortcut.

`tests::objects::sparse_array_query_guard_is_scoped_to_the_intrinsic_realm` exercises customization in the primary realm, default-link restoration in a second realm, inherited-hole visibility through ordinary `HasProperty`, and restoring the primary chain to re-enable the fast query. The focused test passed, and `mbx test -p kiln-interpreter --jobs=1 -- --test-threads=1` passed 263 tests, with 0 failures and 1 ignored. The native helper has no corresponding cached bit: `crates/native/kiln-backend-c/src/runtime/methods.c::kiln_c99_array_next_present_index` checks the active realm's prototype stores on each query. The native and Test262 artifacts above predate this interpreter-only follow-up and are not claimed as reruns of it.
