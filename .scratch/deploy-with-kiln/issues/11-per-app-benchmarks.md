# 11. Per-app benchmarks against Node and Bun

Status: ready-for-agent
Blocked by: 02

Extend `bench/server/compare.sh` from hello-world and one small service to every corpus app that
matches Node. Per app and runtime:

- resident memory at idle and under load (the headline number);
- time from process start to first served response;
- requests per second, p50 and p99, per core and whole machine;
- binary or deployable size (kiln binary vs `node_modules` + runtime).

`CLAUDE.md`'s benchmarking rules apply: time the work, not the launcher; interleave runs; take the
median of several passes; and a response that differs from Node's is excluded and named, never
timed. Apps with a database measure against the same local service for every runtime.

## Acceptance

- One command reproduces the whole table.
- Raw results and machine details are kept beside each published table.
