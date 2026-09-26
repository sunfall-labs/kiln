# 07. `KILN_WORKERS` thread mode, and supervision for process mode

Status: ready-for-agent
Blocked by: 05 (thread mode); none (supervision)

**Thread mode.** `KILN_WORKERS=N` runs N realms of the program in one process instead of forking,
with accepts balanced across realms in process rather than by `SO_REUSEPORT` hashing. Once 08
lands, the realms share one copy of the built-ins. Process mode stays available
(`KILN_WORKERS_MODE=process`, name to settle) for crash isolation.

**Supervision for process mode**, which the launcher lacks today (`kiln-native/src/lib.rs`):
forward `SIGTERM` and `SIGINT` from the parent to every worker so graceful shutdown works in a
container; make workers exit when the parent dies; restart a worker that crashes; and give each
worker its index, so a program can run once-only startup work in one of them.

The `--workers` build flag decided in deploy issue 10 records `auto` or a count in the binary;
because the launcher's own comment warns that a baked count ties a binary to the build machine,
`kiln build` warns when a number rather than `auto` is recorded.

## Acceptance

- `docker stop` on a `KILN_WORKERS=auto` server drains every worker, in both modes.
- Killing one worker in process mode: it is restarted and the server keeps its capacity.
- Thread mode serves the small-service benchmark at least as fast as process mode, and reports its
  memory next to it.
