# ADR 0031: The realm takes its clock, entropy and locale as capabilities

## Status

Accepted. The capability vocabulary, deterministic defaults, HostOps identifiers, and
artifact-policy identity are implemented. Clock consumption is implemented on interpreted and
native paths: civil-time APIs use explicit `HostOp::ClockNow` only for a host-clock realm, while
Tokio's virtual clock remains the scheduler and monotonic source. Entropy consumption is also
implemented on both paths: deterministic and seeded realms use realm-local SplitMix64 state,
while host-backed draws cross `HostOp::EntropyU64`. Timezone and locale are resolved once when an
interpreted realm is entered: named values are validated directly and host values cross
`HostOp::TimeZone` / `HostOp::Locale`; `Intl.DateTimeFormat` consumes the realm zone, while number,
collation, and date-time formatters consume the realm locale. Their deterministic, named, and host
policies also participate in native artifact identity. Full local-`Date` zone semantics, the rest
of the Intl service constructors, native execution of those surfaces, and scripted deterministic
networking remain migration work; live interpreted Node networking already uses the production
Tokio provider without weakening the default realm.

## Context

Four decisions gave the realm a deterministic environment, one axis at a time.

- **ADR 0012** put a virtual clock on the machine. It advances only when a timer is due, and only
  to that timer's due instant, so a program's callback order is a function of the program alone.
- **ADR 0015** made `Date.now()` and `new Date()` read that same clock, starting at the epoch, and
  fixed the realm's timezone at UTC. An unqualified `new Date()` is 1970-01-01, "which is
  obviously not the real time, and is meant to be".
- **ADR 0026** made `Math.random` a SplitMix64 generator seeded with a fixed constant, identical
  in the interpreter and the native backend.
- **`kiln-intl`** made `locale::DEFAULT` a constant, and reads neither `LANG` nor `LC_ALL`, so a
  build that formats a number produces the same bytes on every machine.

Every one of them was right, and together they are the strongest property this compiler has: the
same inputs produce the same bytes, so an artifact hash means something, a race test repeats, and
the differential harness has an oracle.

They also make it impossible to write a server.

That is not a discovered flaw. ADR 0015 named the cost precisely — "two things a program can
legitimately want are now impossible, and both are deliberate" — and both ADRs wrote down what
reversing them would take:

> Nothing here reads the host clock at all, so there is no hidden dependency to remove later if the
> decision is revisited — only one to add, in one place, deliberately.
> — ADR 0015

> Adding a host-supplied seed later changes the constant into a parameter and nothing else.
> — ADR 0026

This is that addition, made once for all four axes rather than four times as each becomes urgent.

## Why now

`docs/design.md` §14 puts the Node compatibility graft at step 10, and
`docs/language-coverage.md` has the ladder at a rung well below it, so nothing here is needed
this month. Deciding it anyway is a deliberate choice against the alternative, which is deciding
it under pressure.

The pressure will be specific and it will arrive in the wrong order. The first program that wants
the real clock will be an HTTP server that has to emit a `Date:` header, and the cheapest thing to
do at that moment will be to make `Date.now()` read the host clock "just for the native backend",
or "just when a flag is set" — and the property that makes every artifact hash meaningful will
have been traded for a header field, in a diff whose title mentions HTTP. `conformance/server/`
exists to make that moment visible early; this decides what to do when it comes.

The four probes in `conformance/server/` are also written so they cannot be passed by cheating:
none of them prints a response header the probe did not itself set. That was cheap to arrange
while nothing runs, and it stops being arrangeable the moment something does.

## Decision

### Determinism is the default, and it is a property of the realm's inputs

The virtual clock, the fixed seed, UTC and `locale::DEFAULT` stay exactly as ADRs 0012, 0015,
0026 and `kiln-intl` specify them. They stop being constants compiled into the engines and become
the **default values of four realm capabilities**:

    clock     virtual (ADR 0012's machine clock, starting at the epoch)   | host
    entropy   SplitMix64 seeded 0x9E3779B97F4A7C15 (ADR 0026)             | host | seeded(n)
    timezone  UTC (ADR 0015)                                              | host | named zone
    locale    locale::DEFAULT (kiln-intl)                                 | host | named tag

A realm constructed with no capabilities named is byte-for-byte the realm that exists today.
Every existing test, the conformance corpus, the differential harness and every artifact key keep
their current meaning, and the change is inert until something asks for it.

### The host values arrive through HostOps, not through ambient reads

No engine crate gains a call to the host clock, to `getrandom`, or to `LANG`. A host-backed
capability is an operation on the `HostOps` boundary (ADR 0007), reached by the same
append-only numeric operation identifiers as every other native operation, and supplied by the
runtime provider.

This is what keeps the reversal honest. ADR 0015's guarantee was that there is no hidden
dependency to remove — one ambient `SystemTime::now()` inside `kiln-interpreter` would end that
permanently, and it would be invisible in review. An operation on a versioned boundary is not
invisible: it appears in the HostOps table, the C prelude, and the capturing test host.

### A realm that reads the host says so in the artifact identity

An artifact compiled for a realm with any host-backed capability records that in its cache key,
and its key is not the key of the deterministic realm's artifact. ADR 0009's layered artifact keys
already carry what a build depended on; a build that depended on the world is a different build,
and the cache must not be able to answer one with the other.

The artifact records capability **policy**, not a transient host value. `clock=host` and
`locale=host` distinguish the compiled contract from the deterministic contract; the wall-clock
millisecond or locale returned during one execution does not create a new compiled artifact.
Caller-selected values such as `entropy=seeded(n)`, a named timezone, or a named locale are part of
the policy and therefore part of the key.

The reproducibility claim then narrows honestly rather than dissolving: **a deterministic realm's
artifact is reproducible, and a host realm's artifact is reproducible in its compilation and not
in its execution.** Both are true statements. "kiln builds are reproducible" with a silent
exception is not.

### The corpus keeps the deterministic realm

`conformance/`, `scripts/differential.sh` and Test262 execution run in the deterministic realm,
always. The differential harness cannot compare against node's real clock by construction — ADR
0015 already said so and already pins `TZ=UTC` — and a corpus that could opt into host time would
have tests whose expected output was a fact about the machine that recorded it.

The consequence for `conformance/server/` is stated rather than left to be discovered: those four
probes are expected to pass in the *deterministic* realm. A server that only works when it can
read the real clock is a server this compiler has not finished.

Deterministic I/O uses the same Tokio-owned runtime with a scripted HostOps provider. The script
owns request identities and completion payloads and releases them at explicit realm checkpoints;
it does not replace Tokio with a mock executor. Live Node execution selects the production Tokio
filesystem/socket provider. Thus the corpus can exercise server state machines without binding a
real port or making kernel completion order observable.

Capability policy and host-provider policy are orthogonal. The former selects observable clock,
entropy, timezone, and locale and participates in artifact identity. The latter selects how an
execution obtains external services; it is chosen explicitly when the realm is constructed and is
never inferred by a builtin. A deterministic realm may run Node server state machines against a
scripted provider, and a live provider may be combined with seeded entropy or a named timezone.

Both providers implement the same request/resource/completion protocol. A script validates the
operation and its arguments, uses ordinary opaque identities, models cancellation and partial I/O,
and releases owned completions through the realm's sole completion ingress. It does not mock
`node:http`, streams, EventEmitter, or the module graph. Those semantics are therefore tested once
and run unchanged over scripted and Tokio-backed services.

## Alternatives rejected

**A global `--deterministic` / `--real` flag.** One bit for four independent axes, and the wrong
granularity for every one of them: a program that wants a real clock rarely wants unseeded
randomness, and a test that wants a fixed seed usually still wants a named timezone. It also puts
the decision on the command line, where an artifact key cannot see it without the flag becoming
an input anyway — at which point it is a capability with a worse name.

**Reading the host by default and offering determinism as a test mode.** This is what every other
runtime does, and it inverts which of the two states is auditable. The failure mode is not that a
program reads the clock; it is that a program reads the clock *and nobody notices*, because
ambient access leaves no trace at the boundary. Defaulting to determinism means the exception is
the thing that has to be written down.

**Leaving it until step 10.** Rejected above, under *Why now*.

## Consequences

- The compiled-server target stops being in tension with the determinism ADRs. Both survive: the
  server names the capabilities it needs, and everything that does not name them is unchanged.
- `Date.now()` differences remain zero outside a timer in the default realm, so ADR 0015's note
  that `kiln-bench` measures from Rust still holds and still needs to.
- The HostOps table grows four operations that native artifacts can target, which is the seam
  working as intended rather than an exception to it.
- Artifact keys acquire a realm-capability component. Until any capability is used, that component
  is constant and no existing key moves.
- `bench/results/server-baseline.json` records what node, deno and bun cost to ship the same
  program, measured before kiln could run it. The comparison this ADR unblocks therefore has a
  baseline column that predates any incentive to flatter it.
- The obvious next question — whether a host-backed realm may participate in the artifact *cache*
  at all, or only in a build — is deliberately not answered here. It needs the cache key change to
  exist first, and answering it now would be inventing a constraint for code nobody has written.
