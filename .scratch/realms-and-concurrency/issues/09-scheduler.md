# 09. The scheduler: turn migration, pinning, deterministic mode, one pool

Status: ready-for-human
Blocked by: 05, ADR 0070 accepted

ADR 0070's scheduler:

1. **One pool.** Scheduler threads, one per core, run realm turns and ADR 0069's parallel chunks
   from one work-stealing queue. The separate Rayon pool in the provider is removed. Tokio stays the
   reactor and wakes realms.
2. **Turn migration.** A realm's next turn may run on any scheduler thread. The mid-turn collector
   records the stack base at the start of each turn, not once per realm.
3. **Pinning.** A realm that loads a native addon, registers an FFI callback, or calls
   `Atomics.wait` is pinned to its thread from then on.
4. **Blocking hand-off.** A turn that blocks the thread hands its scheduler slot to a new thread,
   as Go does for system calls.
5. **Deterministic mode.** Every realm on one thread under the virtual clock, for tests.

A human reviews the design first: it is the runtime's largest change since ADR 0028.

## Acceptance

- 10,000 idle realms in one process, and a benchmark with one realm per connection, run with the
  thread count at cores plus the reactor.
- The 05 harness passes with migration on, and identically in deterministic mode.
