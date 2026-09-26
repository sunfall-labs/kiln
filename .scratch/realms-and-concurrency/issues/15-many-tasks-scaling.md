# 15. Many concurrent tasks in one realm: time and memory

Status: ready-for-agent

The baseline's many-task row: 100,000 async tasks each doing 10 timer ticks take 2.9 s and 732 MB
peak RSS in one kiln realm, against node 1.2 s / 418 MB, bun 0.57 s / 196 MB and Go 0.30 s /
208 MB. Each pending task costs several kilobytes and each tick is slow.

Profile the timer table, promise reactions, async function frames and the nursery behaviour under
this load, and fix what dominates. This matters beyond benchmarks: a server with many idle
keep-alive connections or many scheduled jobs is exactly this shape, and ADR 0070's goal of a
realm per actor makes per-task cost inside a realm matter as much as per-realm cost.

## Acceptance

- The 100,000-task row at or below node's time and memory, natively; interpreter no worse than
  today.
- `mise run native-stress` clean on the probes added.
