# 03. Implement ADR 0040: the executor split

Status: ready-for-agent

ADR 0040 is accepted and not implemented. `RealmRuntime` owns both the Tokio executor and the realm
state, so the native drive re-enters the executor about 8.5 times per request and carries an
`_in_context` twin for eleven methods. Split them as the ADR decides: one executor entry per program
run, the drive holding `&mut RealmState`, and the `_in_context` family deleted.

This is a single-realm throughput change first. It is also the first step of ADR 0070: once realm
state is its own value, a realm can be driven by any thread.

## Acceptance

- The `_in_context` methods and the `in_context` forks in the adapter are gone.
- Runtime entries per request, measured as in ADR 0040, fall to about one.
- `scripts/native.sh` 0 diverged; the small-service benchmark does not regress.
