# 05. Several pinned realms in one process

Status: resolved
Blocked by: 03, 04

With realm state separate (03, 04), run several realms in one native process, each pinned to its
own thread: the 1:1 model, which ADR 0070 keeps as pinning. `WorkerPool` in `kiln-runtime` is the
starting point; make it the realm host rather than a parallel implementation.

Test with a harness that runs N copies of each `conformance/native` probe as separate realms
concurrently in one process and requires each to print exactly what it prints alone.

## Acceptance

- The harness passes for N = 1, 2 and the core count, and under `KILN_GC_STRESS`.
- Nothing in the process-global inventory from 01 remains unclassified.

## Comments

### 2026-09-27 -- done (branch `work/concurrent-realms`, with issue 04 step 4)

**The host is `WorkerPool`.** It gained one thread-spawning path (`spawn_thread`: capacity, the drop
guard, the `kiln-worker-N` name) that both kinds of realm it hosts go through: the mailbox-served
`RealmRuntime` it had, and `spawn_pinned` -- a realm that makes, enters, drives and destroys itself
on its own thread and is joined as a `PinnedRealm`. A native realm owns its executor and cannot run
inside a worker's `block_on`, which is why it is not a mailbox task. The provider exposes it as
`kiln_realm_run_pinned(count, run, context, statuses)` (ABI 46): one pool sized to `count`, each
index's `run` on its own worker thread, all joined. The provider's realm registry was already per
thread, so each realm lives where it was made (ADR 0070's pinning).

**The launcher.** `KILN_REALMS=N` (read like `KILN_WORKERS`) makes the graph launcher run N realms of
the program at once through that call. Each realm's standard streams go to
`$KILN_REALMS_OUT/<index>.out`, both into one file as `2>&1` does, and its exit status to
`<index>.status`: `kiln_cli_host_state` gained `output` (console writes and failure reports go
there), and the realm context gained `output_fd`, which `process.stdout/stderr.write` use for fds 1
and 2 (`kiln_c99_realm_capture_output`, set inside the realm before any module runs). A realm run
now *returns* node's 13 for an unsettled top-level await instead of calling `exit`. Embedding
runtime version 12 -> 13 (`kiln_cli_warn` takes the host state; the header includes `<stdio.h>`).

**The harness** is `scripts/native-realms.sh` (`mise run native-realms`; documented in AGENTS.md):
each probe is built once by `kiln run --native` (so the realm keeps its virtual clock; `kiln build`
binaries keep wall time), run alone, then with `KILN_REALMS` = 1, 2 and the core count, and every
realm's output and status must equal the lone run's. It takes probe paths to run a subset and
`KILN_NATIVE_GC_STRESS=1` for the stressed variant. It skips, and counts, `process.exit` probes
(that ends every realm; a realm's own exit is issue 06's) and probes writing fixed paths (the
realms share a filesystem). A test in kiln-native was the alternative; a script matches how
`native.sh` drives the same corpus and keeps a per-probe build out of the crate's test time.

**Results** (final tree, debug `kiln`, 16 cores): `scripts/native-realms.sh`, N = 1, 2, 16: 241
matched, 4 skipped (shadow-realm unbuilt, app-csv-report exits, node-fs-members and
shape-keys-survive-collection write files), 0 diverged, 2m47s. Under `KILN_NATIVE_GC_STRESS=1`, N = 1, 2, 16, over
eleven cache- and collector-heavy probes (accessors, array-methods, biased-dictionary-cache,
builtin-static-method-cache, classes-and-callables, closures-in-loops, collections,
define-site-cache, json-shaped-build-edges, negative-cache-invalidation, object-literal-shapes): 11
matched, 0 diverged, 45 s. (message-port-transfer also matched under stress in an earlier run of
this tree's predecessor but takes tens of minutes at 16 stressed realms, so it is out of the
subset.)

What the harness found on the way, all fixed in this branch (details in issue 04): a site handing
another realm's transition into its realm-local shape (`object-literal-shapes`); racing realms
claiming a receiver row each for one shape, then the typed entry refusing (class-poly at 12+
realms); a typed body refusing while *another* realm bootstrapped (the built-in store summary); a
teardown reading an entry's key word twice; and a receiver-kind hint trusted without the kind
(`negative-cache-invalidation` under stress; also wrong on main in one realm).

**Scaling note.** 16 realms of a property-heavy loop on 16 hardware threads (8 cores, SMT) run the
same instructions per realm as one alone and about 2.3x the cycles, with other agents' load on the
machine; class-poly: 238 ms alone, 477 ms for 16 at once.

**Inventory.** Nothing process-wide is unclassified: every mutable prelude static -- now including
the function-scope ones, renamed `kiln_c99_*` so the manifest sees them -- has a
`C99_MUTABLE_STATE_OWNERS` row, every emitted one an `EMITTED_MUTABLE_STATE_OWNERS` row, both
enforced by tests; issue 04 step 4 lists how each shared one is written.

**Left.** `process.exit` in one realm ends the process (issue 06). `KILN_WORKERS` still forks
(issue 07). A module loaded after realms exist runs in one live realm at a time (refused in a
second). The scheduler (issue 09) will move realms between threads at turn boundaries; the frame
registers, streak table and heap cache are per thread and rely on a realm holding no generated
frame across a boundary, as issue 04 step 5 notes.

