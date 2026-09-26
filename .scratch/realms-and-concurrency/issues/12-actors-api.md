# 12. Actors: the API

Status: needs-triage
Blocked by: 09, 10

Decide, in an ADR, the actor built-ins: an actor as a realm (ADR 0070), how it is spawned and
addressed, how calls become messages, what `isolate` and `transfer` mean as built-ins, and what
becomes of `@kiln/concurrency`. The target granularity is an actor per connection or per user, which
09's realm cost has to support.

## Acceptance

- An accepted ADR; `@kiln/concurrency` either re-exports the built-ins or is retired.
