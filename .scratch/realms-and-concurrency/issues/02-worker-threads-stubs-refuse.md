# 02. `worker_threads` stubs refuse instead of doing nothing

Status: ready-for-agent

`crates/kiln-builtins/src/node/worker_threads.ts` defines a fallback `BroadcastChannel` whose
`postMessage` does nothing and a fallback `MessageChannel` whose ports are empty objects. A program
that uses them gets silently wrong answers, which is the failure `CLAUDE.md` calls the expensive one.
Until 06 lands, each should refuse naming the member, as `Worker` already does.

## Acceptance

- Using the fallback `BroadcastChannel`, `MessageChannel` or `MessagePort` throws a refusal naming
  the member, in both engines.
- A `conformance/native/*-refuses.ts` probe records the decision until 06 replaces it.
