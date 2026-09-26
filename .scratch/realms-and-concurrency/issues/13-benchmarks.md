# 13. Benchmarks against Node, Bun and Go

Status: ready-for-agent
Blocked by: 01 (then extended as each issue lands)

Under `bench/`, following `CLAUDE.md`'s rules (warm, interleaved, checksums compared):

- worker start time and memory: a kiln realm, a Node worker, a Bun worker, a goroutine;
- message throughput and latency, small and large payloads, copy and transfer;
- sharing a large read-only dataset across N workers;
- many-realm scaling: 10, 1,000 and 100,000 realms or goroutines doing a small periodic task;
- server scaling: thread mode against process mode against Node `cluster` against Go `net/http`.

## Acceptance

- One command reproduces the table, with machine details, and it is updated as each issue lands.
