# 13. Parallel code in every module and every build profile

Status: resolved
Priority: first, before tiers 2-4

Found by issue 03 (`--explain-parallel`, reason `generic-only`): `kiln build` defaults to the dev
profile, which never emits typed or parallel code, and a release-profile module with more than 16
functions or 512 instructions is also emitted generic-only. So most real modules get no parallel
`parallelMap` whatever their callbacks, and every later tier multiplies nothing.

Find why the typed/direct emission is gated on module size and profile (compile time, code size,
the direct plan's cost), and make the parallel lowering of a qualifying `parallelMap` call site
independent of it: the callback and the call site need typed entries, not the whole module. Decide
whether the dev profile should emit parallel code at all (it is the edit-run loop; the interpreter
is the reference there), and record the decision.

## Acceptance

- A `parallelMap` call site with a qualifying callback runs in parallel in a large release module
  (a probe with more than 16 functions and 512 instructions).
- `--explain-parallel` no longer reports `generic-only` for release builds of the concurrency
  benchmark and the deploy corpus apps.
- Release build time of the thirdparty bundles within 10% of before.

## Comments

2026-09-26, branch `work/parallel-every-module`.

**What gated it.** Two switches, both about build cost, neither about parallelism.
`kiln-native`'s `release_module_uses_generic_bodies` (`MAX_RELEASE_SPECIALIZED_MODULE_*`: 16
functions, 512 retained instructions, from 9622cd91 "Accelerate release graph builds") sets
`EmissionPolicy::generic_only` for a large release module, and that module is then also
compiled at `-O0` without function sections: cloning every body (typed, fast, speculative)
multiplies emission, C parse/optimize time and object size for large bundles. `native-dev`,
`check` and `native-release-size` set `generic_only` unconditionally. `DirectPlan::plan`'s
generic branch then skipped every analysis -- known callees, the numeric fixpoint, typed
entries, and so the `parallelMap` verdicts -- and `parallel_site` refused every site.

**What changed.** A generic-only plan now plans the sites and nothing else
(`DirectPlan::plan_parallel_sites`): one scan of the unit for one-argument `parallelMap` calls
(a unit with none, i.e. every thirdparty bundle, stops there); then `known_callees`, and for each
callback proto the full plan's verdict step for step (source rules, typed eligibility, the
return proven a number with parameters and captures seeded) and, where it qualifies, its typed
entry and `_p` clone plans. Every other body stays generic; the only fact added to a generic
body's plan is the callback value of its own site (left out if the same value is also called
directly, so no `Call` changes lowering). `lib.rs` emits the typed entry, clone and chunk
workers from those plans exactly as before (the clone's text check still applies). At the site,
the generic `CallMethod` path now emits the pooled lowering first (factored out as
`parallel_pool_lowering`, shared with the specialized path, including the new detached-view
guard) and the generic call only when it did not answer. Policy: `EmissionPolicy::parallel_in_generic`,
set by `kiln-native` for every profile except `native-dev` and `check` (so also
`native-release-size`); mixed into the unit hash only when it changes emission.

**Dev profile: no parallel code, by decision.** `native-dev`/`check` are the edit-run loop:
they emit no typed code at all and compile for build speed, so a pooled site there buys little
(the workers would be unoptimized too) and costs emission and compile time on every edit, and the
answer is byte-identical either way -- the interpreter and the release harness are where results
are checked. What was wrong was the explanation: those sites now say
"dev-profile: this is a development build, which emits no parallel code; build with
`--profile native-release`" instead of `generic-only`. `generic-only` is kept as a code (older
records parse) but no profile produces it.

**Acceptance.**
- New probe `conformance/native/parallel-large-module.js` (~40 functions, well past 512
  instructions; `profile=native-release parallelism=1,2,max`): base (f93847a0) reports every site
  `generic-only`; now 5/7 `parallel`, the other two `captures-reassigned` and `reads-property`.
  Binary output equals the interpreter at `KILN_PARALLELISM=1,2,16`.
- Speed, same heavy callback (2000-step loop, 200k elements), release: large (generic, `-O0`)
  module `parallelMap` 9016 ms at 1 thread / 823 ms at 16, vs `map` there 35-41 s; small
  (specialized, `-O2`) module 3307 / 261 ms. So a generic module's workers run about 2.7x slower
  per element than a specialized module's -- they are compiled at the module's `-O0`. Follow-up:
  emit a generic unit's `_p` clones and chunk workers into their own `-O2` translation unit.
- `--explain-parallel`, release: `bench/concurrency/js/pmap-parallel.mjs:16:21 parallel`,
  `pmap-parallel-light.mjs:17:21 parallel`, `bench/non-io/multicore-parallel.mjs:19:28 parallel`,
  `bench/non-io/parallel-map-large.mjs:20:26 parallel` (these were small enough to be parallel
  before too). There is no deploy corpus in this repository to check.
- Release build time, typescript/babel/lodash bundles (`--one-shot --node build --profile
  native-release`, fresh cache per run after a warm-up build, release `kiln` at base and at this
  branch, ABBA order). Generated C and app object are **byte-for-byte the same size** in every run
  (lodash 9,860,903 / 16,831,448; babel 39,392,681 / 61,566,928; typescript 237,136,606 /
  293,332,824) -- no bundle has a `parallelMap` site, so the only added work is the one scan.
  The timings (emit / compile, ms) are dominated by machine load (load average 24-30):

  | bundle | base 1 | new 1 | new 2 | base 2 |
  | --- | --- | --- | --- | --- |
  | lodash | 1107 / 18908 | 892 / 16290 | 994 / 21336 | 1211 / 90798 |
  | babel | 2866 / 14831 | 7162 / 35577 | 4025 / 22452 | 8686 / 49800 |
  | typescript | 23741 / 50348 | 30474 / 109231 | 39112 / 81280 | 48106 / 66695 |

**Verified.** `mbx test -p kiln-backend-c --offline`: 210 + 36 passed, 0 failed (new:
`reason_dev_profile`, `a_generic_only_unit_still_lowers_its_qualifying_sites`,
`a_generic_only_unit_runs_its_qualifying_sites_in_the_pool` -- the pool fixture counts that every
site took the pool, and results equal `map` bit for bit). `scripts/native.sh`: 238 matched,
1 refused (`shadow-realm`, pre-existing), 0 diverged. `native-release-size` checked by hand on
`parallel-capture-const.js` (6/7 parallel, output equal at 1 and 16 threads).
`mise run native-stress` not run.
