# 06. `node:worker_threads` on realms

Status: ready-for-agent
Blocked by: 05

Implement `node:worker_threads` with Node's semantics, a worker being a realm (ADR 0070):
`Worker` (from a file, from `eval`, with `workerData`, `env`, `resourceLimits` as far as they apply),
`parentPort`, `isMainThread`, `threadId`, `MessageChannel`, `MessagePort`, `BroadcastChannel`, the
structured clone algorithm, transfer lists (copying at first; 10 makes them zero-copy),
`SharedArrayBuffer` and `Atomics` across realms, `worker.terminate()`, and error and exit events.
What `process` looks like inside a worker follows Node's rules for workers.

Written once in TypeScript over declared primitives (ADR 0065), with the realm and port machinery in
the runtime.

## Acceptance

- The deploy corpus's pino-with-transport case and a piscina case match Node.
- Node's own `worker_threads` behaviours covered by probes in `conformance/node`, run with `--node`.
