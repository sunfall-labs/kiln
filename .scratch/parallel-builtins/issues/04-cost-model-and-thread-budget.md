# 04. Cost model and thread budget

Status: ready-for-agent

**Cost model.** Today `kiln_parallel_map` in the provider splits work into fixed-size chunks
(`CHUNK`) whenever the callback qualifies. Add a threshold on element count and estimated work per
element below which the call stays sequential, and size chunks from the measured cost of the first
chunk. Goal: no input size where `parallelMap` is measurably slower than `map`.

**Thread budget.** With `KILN_WORKERS=N`, every process builds a full pool today, so the machine
runs N × cores threads. Default the pool to `max(1, cores / N)`; `KILN_PARALLELISM` still
overrides it. The launcher passes N to each realm.

## Acceptance

- A benchmark sweeping input sizes from 1 to 10 million shows `parallelMap` never slower than
  `map` by more than measurement noise.
- `KILN_WORKERS=auto` on a many-core box runs at most one pool thread per core in total.
