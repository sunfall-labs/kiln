# 24. A segfault with several realms in one process

Status: resolved
Priority: first

Found by issue 08: on origin/main `3ffad3e4`, the `hold` binary (`console.log("hi")` plus a 3 s
timer, the issue 01 measurement program) segfaulted 3 times in about 90 runs at `KILN_REALMS=4`.
The shared-region branch did not crash in about 140 runs, which does not show it fixed. A crash that
depends on timing between realms is exactly what issue 04 step 4's slot publication and reclamation
must rule out.

Reproduce it reliably (a loop of runs, more realms, `taskset` to vary scheduling, a debug build with
the core dump analysed — see the `diagnose-crash` approach: `coredumpctl`), find the race, fix it at
its cause, and add a stress mode to `scripts/native-realms.sh` that repeats a probe many times at
high N so this class is caught.

## Acceptance

- The cause identified and fixed; 1,000 runs of the repro at `KILN_REALMS=4` and `=16` without a
  crash; the new repeated-run mode in `scripts/native-realms.sh`.

## Comments

Resolved 2026-09-27. Core PID 1983270 (`hold50_rel`, `SIGSEGV`) shows a realm thread in the
allocator computing an address from a zero block size while another realm thread is in allocator
teardown. The source confirmed the race: `NativeRealm::drop` released callable stores and ran GC
teardown after its active heap had been removed from thread-local state. Prelude callbacks then
attached to one process-wide fallback heap; concurrent realm threads initialized and used that
heap without synchronization. The zero-size allocation is direct core evidence; the shared
fallback mechanism is established by the teardown and heap-selection paths.

Teardown now makes the realm's own heap current through its destructors and GC hooks, then clears
it after the heap is freed and invalidates the prelude's cached heap answer. The fallback used by
prelude fixtures or failed attachments is thread-local. A provider test checks the heap visible
during callable-store destruction, collector hooks and root reset. The prelude ABI snapshot was
recorded with `INSTA_UPDATE=always`.

`scripts/native-realms.sh --repeat K` builds once and repeats each requested realm count, reporting
the run and signal on a crash. The exact short repro is
`docs/issues/realms-and-concurrency/probes/teardown-timer.js` (`"hi"` plus a virtual 3 s timer).
It passed 1,000 runs at N=4 and 1,000 at N=16: 1 matched, 0 skipped, 0 diverged. The existing
`conformance/native/timers.ts` also passed 1,000 runs at both counts. Focused tests: backend C
215 unit and 36 emitted-C passed; native provider 38 passed; runtime 168 passed.
