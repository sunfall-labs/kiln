# 12. Actors: the API

Status: resolved (module-addressed API and package migration implemented; graph moves remain issue 10)

Decide, in an ADR, the actor built-ins: an actor as a realm (ADR 0070), how it is spawned and
addressed, how calls become messages, what `isolate` and `transfer` mean as built-ins, and what
becomes of `@kiln/concurrency`. The target granularity is an actor per connection or per user, which
09's realm cost has to support.

## Acceptance

- An accepted ADR; `@kiln/concurrency` either re-exports the built-ins or is retired.

## Comments

### 2026-09-28 -- plain actor calls and replies avoid a temporary TypeScript graph copy

`ActorRef.call` previously walked and copied every argument graph in TypeScript to find
transfer tickets, then `postMessage` encoded that copy again. A native read-only certificate
now recognizes a complete graph of ordinary shaped objects and dense arrays with plain values,
including shared references and cycles. It refuses accessors, proxies, ticket or isolated
handles, unsupported kinds, special descriptors and realms whose built-ins changed. A certified
call passes its original arguments to the existing shared message codec, which still makes the
one receiver-owned copy. Resolved actor replies use the same certificate. Any refusal uses the
unchanged ticket-aware TypeScript path. Actor spawn keeps its prior path.

Seven ten-pair runs of the public 1 MB actor benchmark on the native-release binary produced
matching checksums and a 12.35 ms median for ordinary clone, down from 75.67 ms before this
change (`fed40541`). A copied 1 MB graph round trip measured 19.37 ms; prepared transfer was
10.35 ms end to end. A seven-round alternating
old/new actor-start check against `fed40541` measured 2.423/2.404 ms warm spawn and
0.285/0.289 ms first call; the guarded route did not impose a measured startup penalty. The
new conformance probe checks cycle identity and one read of an accessor and a Proxy through the
fallback in both engines, including replies.

Backend-C tests passed 253 unit and 36 emitted-C cases; built-ins passed 107 and interpreter
203. Final native parity reported 347 matches, one known refusal and zero divergences. Full
collector stress on the initial broader guard reported 307 matches, one known refusal and zero
divergences with 40 declared skips; the final call-and-reply variant also matched the new probe
under GC stress at 1, 2 and 16 concurrent realms. The quick message report rendered every new
actor row without a divergence. Raw measurements and logs are under
`~/.cache/kiln/actor-call-codec-2026-09-28/`.

### 2026-09-27 -- API accepted

ADR 0072 defines module-addressed actors in their own realms, opaque call capabilities, bounded
mailboxes and explicit ownership tickets. The project owner delegated approval and implementation.
The runtime and `@kiln/concurrency` migration remain pending.

### 2026-09-28 -- Runtime and compatibility import

`kiln:actors` now starts an actor module in a worker realm, registers a sealed shared address,
accepts bounded call reservations, transfers `ArrayBuffer` tickets in both directions, reports
remote method errors, and revokes calls at shutdown. A process-wide request sequence prevents
different caller realms from colliding, and an owner-only sealed key prevents a delegated
address from revoking or redirecting an actor. The old `@kiln/concurrency` in-realm proxy and
identity transfer helper were replaced by re-exports; its README describes the source break.

`isolate(factory)` creates an opaque handle, but an object-graph ticket still raises
`DataCloneError` without consuming it. Issue 10 owns the exact-root proof and storage handoff.
Issue 10 subsequently added `prepareTransfer(value)` for supported plain data graphs; it
snapshots the input before a single-use actor handoff in both engines. Direct `isolate(factory)`
still requires its separate ownership proof.
ADR 0072 now requires a URL or absolute path for `module`: a relative string cannot be resolved
from inside a library function without losing the caller's module location.

Focused module-graph integration exercises spawn, delegated calls, a returned buffer transfer,
and revocation in the interpreter and generated native program. The native program also checks
the extension-off diagnostic. The native integration passed with `KILN_NATIVE_GC_STRESS=1`.

The compatibility import also runs without `--node`. A full migrated native-realm sweep exposed
that the CLI source collector tried to read the virtual `kiln:actors` path as a real
`.kiln/builtins/actors.builtin` file when a package re-exported it. The pinned resolver then
failed on the actor module's internal `node:worker_threads` and that built-in's `node:events`
dependency. Both resolvers now recognize kiln namespaces in ordinary mode and permit a virtual
engine-owned built-in to import its own built-in dependencies. Ordinary source modules still
cannot import `node:*` without the Node adapter. The focused resolver/source-snapshot tests and
the `kiln-concurrency-engine-method.ts` interpreter/native probe passed in ordinary mode.
