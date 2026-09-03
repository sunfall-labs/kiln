# ADR 0045: Dynamic compilation throws where it cannot happen, rather than refusing

## Status

Accepted. Amends the Status of ADR 0030, which said the native backend *refuses* dynamic source
until ADR 0036's injected native compiler service exists. It still has no compiler; what changes is
what it answers when a program asks for one. `new Function(source)` now throws an `EvalError` the
program can catch, at the call, instead of ending the process.

## Context

`new Function(source)` compiles source while the program is running. An ahead-of-time pipeline is
arranged not to be able to do that: `kiln-backend-c` emits C and hands it to an external toolchain,
and ADR 0007 keeps the frontend out of the engine on purpose. ADR 0030 records why, and defers a
real answer to ADR 0036's injected compiler service.

So the backend cannot run it. The question this decides is narrower and separate: what a program
that *asks* is told.

It was told nothing it could act on. The refusal ended the process at the call site with a
diagnostic. That is the right instinct in general -- this backend refuses rather than answering
wrongly -- but it is the wrong answer here, for a reason the corpus made visible.

**Real libraries already handle this environment.** A browser under a Content-Security-Policy and
Node under `--disallow-code-generation-from-strings` both refuse to compile strings, and both do it
by *throwing*. Libraries that care are written against that: a bundled `zod` carries a
`ZodCompileUnsupportedError` and a whole second path behind it, thirty occurrences in the bundle.
An uncatchable refusal meant that path could never run. `zod` did not fail because it needed a
compiler; it failed because it was never told it could not have one.

**The claim a refusal makes is too broad.** Refusing says "this backend cannot run this program".
What is true is smaller: this realm has no compiler to give, on this path, if it is taken. A
program that merely *links* a library with an eval path it never enters is unaffected either way --
measured, not assumed: a `new Function` branch that is not taken compiles and runs natively today,
because the refusal was always at the call and never at emission. But a program that does enter it,
and is written to cope, was being stopped for a condition it had already handled.

## Decision

**`new Function(source)` throws `EvalError` with the message
`Code generation from strings disallowed for this context`.**

The kind and the wording are Node's under `--disallow-code-generation-from-strings`. They are not
invented, and that is the point: it is the error libraries already feature-detect on, so the
existing fallback in a library is reached without the library knowing anything about Kiln.

This is not an approximation of `eval`. Nothing is compiled, no second heap is consulted, and no
scope is reconstructed -- the failure modes ADR 0030 refuses to approximate stay refused. The realm
answers a question about itself, truthfully, in a form the caller can catch.

`eval` itself is unchanged: it is still absent by name in the native realm, and ADR 0030's account
of direct versus indirect eval stands. Only the constructor's answer changes.

## Consequences

`zod` compiles and runs natively, matching node byte for byte. `ajv` gets past the same wall to a
much smaller one. Neither needed a compiler.

`EvalError` had to become a real error first, which is the other half of this change and was a
latent bug: `kiln_c99_error_name` -- the function that decides whether a built-in *is* an error --
listed six of the language's seven and omitted `EvalError`. The constant existed, the global
resolved, and `kiln_c99_builtin_name` knew the name, so nothing looked wrong; but `new
EvalError("m")` refused where the interpreter built one, `EvalError.prototype` had no owner, and
`kiln_c99_throw_error` could not raise one at all. That last failure is silent: a throw that cannot
be built returns a plain error status, and the caller reports whatever it was doing at the time.
It cost an hour of looking in the wrong place -- the branch that raised the `EvalError` was correct
from the first attempt and appeared not to run.

The cost is that a program relying on `new Function` now fails later and differently than it would
have: at its own catch site rather than at ours, possibly silently if it catches and ignores. That
is the trade the language itself makes under CSP, and a program that swallows the error was going
to be wrong either way.

A refusal remains the answer wherever no such convention exists. This is not a licence to convert
refusals into throws generally -- ADR 0042 keeps `Intl` refusing by name for exactly that reason.
The distinction is whether the language has an established way for a realm to say "not here" that
programs are already written against. For dynamic compilation it does.
