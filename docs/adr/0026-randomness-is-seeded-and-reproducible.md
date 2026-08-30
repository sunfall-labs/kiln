# ADR 0026: `Math.random` is seeded, and a run reproduces

## Status

Accepted and implemented in both engines.

## Context

`Math.random` was deliberately absent. The comment on the `Math` native said why:

> a nondeterministic builtin needs a seeding decision this compiler has not made, and an unseeded
> one would make every artifact hash a lie.

Both halves of that were right, and neither is a reason to leave it out for good. It is a reason to
make the decision, which is what this records.

The pressure to make it is real: `docs/language-coverage.md` lists `Math.random` among the statics
the Effect bundle names, alongside `Object.assign` and `Array.from`. A compiler that cannot run
`Math.random()` cannot run a great deal of ordinary code, and the failure is not subtle -- it is a
missing method at the moment it is called.

## Decision

`Math.random` returns a value from a **deterministic generator with a fixed seed**, identical in
the interpreter and the native backend.

- The generator is SplitMix64: a full-period generator with good statistical properties in five
  lines, which is what a language builtin needs and is small enough to write twice without the two
  copies drifting.
- The seed is the constant `0x9E3779B97F4A7C15`, per program run.
- The result is 53 random bits over 2^53, which is every bit the mantissa has.

## Why not the environment

**An artifact hash would stop meaning anything.** A build is a function of its inputs; if the
runtime reads entropy, two runs of the same artifact differ and the cache can no longer claim that
the same key means the same behaviour. That is the objection the original comment made and it
stands.

**Both corpora would lose a test.** `conformance/native` compares the two engines byte for byte,
and a probe calling `Math.random` could not be in it at all if the value came from the
environment. With a shared seed it can, and it *does* -- the sequences are checked equal, which is
a real check on both implementations rather than a hole in the corpus.

**It is the same trade the clock already makes.** Timers run on a virtual clock that advances to
whatever is due next rather than waiting; a program that schedules an hour out finishes instantly
and always in the same order. Randomness reproducing is the same decision applied to the other
source of ambient nondeterminism, and the two are better made the same way than differently.

## What this costs

A program that needs *unpredictability* -- a token, a nonce, a salt -- gets a sequence an attacker
can reproduce exactly. That is a real limitation and it is why this ADR says "reproducible" rather
than "random".

It is also not what `Math.random` is for. The specification has never required it to be
cryptographically secure, and every engine's documentation says so; a program that needs
unpredictable bytes is supposed to ask `crypto.getRandomValues`, which this compiler does not have
and which would be the place to put an entropy source when it does. Wiring that through the host --
where an embedder can decide what entropy means -- is the right shape for it, and is exactly the
distinction ADR 0025 draws between a service of the environment and a language operation.

Until then, a program that wants a different sequence can be given one the same way it is given a
different clock: by a seed the host supplies. That is a smaller change than it sounds, and it is
deliberately not made yet -- nothing has asked for it.

## Consequences

- `Math.random()` works, and `conformance/native/numbers.ts` checks the two engines produce the
  same sequence.
- The seed is a shared constant in two places, one per engine. If either moves, the native corpus
  fails immediately and by design -- which is the cheapest possible way to keep two copies honest.
- Adding a host-supplied seed later changes the constant into a parameter and nothing else.
