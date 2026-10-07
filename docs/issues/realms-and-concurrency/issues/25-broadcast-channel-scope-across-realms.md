# 25. `BroadcastChannel` is scoped to one program, not to the process

Status: resolved

Found by issue 08's `scripts/native-realms.sh` run: `worker-threads-broadcast` diverges because a
`BroadcastChannel` created in one top-level realm reaches another top-level realm in the same process.
Top-level realms (`KILN_REALMS`, `KILN_WORKERS` thread mode) are separate programs, like node
`cluster` processes, which never share broadcast channels; only a program's own `worker_threads`
workers share its channels. Scope broadcast groups (and any other process-wide messaging namespace)
to the top-level realm that owns the worker tree.

## Acceptance

- `scripts/native-realms.sh` N=1,2 at 0 diverged including `worker-threads-broadcast`; a probe showing
  two top-level realms with the same channel name do not see each other's messages.

## Comments

Resolved 2026-09-27. Broadcast groups now use `(program ID, channel name)` as their key. Each
top-level program gets its own ID; workers and their descendants inherit it, so channels meet
within a worker tree while top-level realms with the same name remain separate. Port IDs remain
process-wide because a port is reached only through an explicit handle.

The runtime test makes the same named channel in two top-level threads, checks that neither gets
the other's post, then checks delivery from a worker to its parent program. The native
`worker-threads-broadcast.cjs` probe, whose top-level realms all use channel name `probe`, matched
its lone run at N=1 and N=2: 1 matched, 0 skipped, 0 diverged. Runtime crate tests: 168 passed.
