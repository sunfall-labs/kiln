# ADR 0017: An unresolved identifier is a runtime error, not a compile error

## Status

Accepted.

## Context

The compiler resolves every identifier it meets to a local slot, a captured cell, a module
binding, or a known global. Until now, a name that matched none of those was a *compile* error:
lowering refused the module and nothing ran.

That is the right answer for a language with a closed global namespace. JavaScript does not have
one. A bundle written against a browser reaches for `window`, `document`, `navigator`, `fetch`;
one written against node reaches for `process`, `Buffer`, `require`. A well-behaved bundle guards
each of these — `typeof window !== "undefined"`, `try { require(...) } catch {}` — and takes a
different path when the name is missing. Refusing the whole module means the guard never gets to
run, and a program that was written to work in a realm this small is rejected anyway.

Concretely: the Effect bundles are compiled for a neutral target and probe for their environment
on the way up. Every one of them named at least one global kiln does not have, in a branch that
would not have been taken.

The tension is with the project's standing preference for loud failure. A compile-time refusal is
maximally loud: it names the identifier and the file, and no wrong answer can follow it. Deferring
to runtime buys compatibility by giving up some of that.

## Decision

An identifier that resolves to nothing compiles to `ThrowReference(name)`, which raises a
`ReferenceError` naming the identifier if and when control actually reaches it.

Three properties make this a smaller concession than it looks:

- **`typeof` is exempt in the language already.** `typeof undeclared` is `"undefined"` rather than
  a throw, per spec, and that is precisely the operator the guards use. The lowering for `typeof`
  on a bare identifier does not emit the throw.
- **The failure is still loud, just later.** An unresolved name never reads as `undefined` and
  never propagates silently into arithmetic or a property read. It throws, with the identifier in
  the message — the same information the compile error carried, delivered when the program depends
  on it rather than when it merely mentions it.
- **It is where the specification puts it.** An unresolved reference is a runtime `ReferenceError`
  in JavaScript. Making it a compile error was kiln inventing a stricter language than the one it
  claims to implement, which is a debt that comes due on every real-world input.

## Consequences

A module that mentions an unknown global now compiles, and a `--check` run no longer catches
typos in identifier position. That check has to come from a type layer or a lint, which is where
it belongs; the compiler cannot distinguish a typo from a legitimate host global it happens not to
implement, and treating both as fatal is what blocked the bundles.

The instruction is one more opcode with a persisted hash tag, and one more thing `verify` must
account for. Nothing else in the pipeline changes: the name is a constant, the throw is an
ordinary catchable error, and a `try`/`catch` around a probe behaves exactly as the program's
author expected.

The refusal-driven backlog loses one of its entries. Previously an unimplemented global surfaced
as a compile refusal, which is how most of this engine's gaps were found. Now it surfaces as a
runtime `ReferenceError` from the differential harness, which is a *divergence* rather than a
refusal — a louder classification, not a quieter one, since divergence fails the build and a
refusal does not.
