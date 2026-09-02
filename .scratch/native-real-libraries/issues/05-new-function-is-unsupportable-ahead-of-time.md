# `new Function` blocks `zod` and `ajv`, and always will

Status: decided in principle; implementation blocked

Both bundles refuse on the same thing, and the refusal now names it:

    zod   operation=New detail=New target is unsupported by the native constructor model
          function=Function receiver_tag=8
    ajv   same

`new Function(source)` compiles source text at run time. The native backend emits C ahead of time
and hands it to an external toolchain; there is no compiler in the running program, and ADR 0007
says the interpreter owns no parser either. So this is not a gap that can be filled by implementing
a member. It is a property of the strategy, and refusing is the honest answer.

That makes it worth deciding rather than leaving on a backlog it can never come off.

## Why it may still be worth doing something

`zod` ships a `ZodCompileUnsupportedError` -- it appears thirty times in the bundle -- which means
the library already has a path for "this environment will not compile functions for me". Browsers
under a Content-Security-Policy are exactly that environment, and libraries that care are written
to fall back when `new Function` *throws*.

A refusal is not a throw. It ends the process, so the fallback never runs. If `new Function` threw
a catchable error instead, `zod` would very plausibly take its own documented path and work.

## The decision

Refusing and throwing are both defensible and they are not the same claim:

- **Refuse** says "this backend cannot do it", which is true, and stops the program at the exact
  spot with a diagnostic naming `Function`.
- **Throw** says "this environment does not allow it", which is also true and is what a CSP-locked
  browser does. It is catchable, so a library's fallback runs -- and a library without a fallback
  gets an error it can report rather than a process that stopped.

The second is more useful and is not a wrong answer, but it is a semantic choice about what the
native realm *is*, which is ADR-shaped. ADR 0042 made the same shape of call for `Intl` and chose
refuse-by-name; the difference here is that real libraries feature-detect on this one, and the
whole point of the exercise is real libraries running.

Do not implement either quietly. Write the ADR.

## What this does not cover

`lodash` refuses on `new DataView`, which is an ordinary unimplemented builtin and not this
problem. See issue 04.

## The decision, after a second opinion

A peer session argued for the throw and cited ADR 0017 (an unresolved identifier is a runtime
error) and ADR 0030 (`eval` takes an injected compiler): the native realm's answer is "no compiler
was injected", which is a condition of the realm rather than a defect in the program, so it should
be a catchable error at the call site.

Two corrections to that argument, both measured here rather than assumed:

- **It is not a compile-time refusal, and an unused eval path costs nothing.** A program with a
  `new Function` branch that is never taken compiles and runs natively today. The refusal happens
  at the call, so "it punishes every program that merely links a library with an unused eval path"
  is not true of the current implementation.
- **ADR 0030's Status says the native backend refuses dynamic source** until ADR 0036. So this is
  an amendment to an accepted decision, not an application of one, and the ADR has to say so.

The argument survives both corrections, for a narrower reason than the one given: the refusal is
*uncatchable*, so a library's fallback cannot run. `zod` carries a `ZodCompileUnsupportedError`
for exactly this environment and never reaches it.

The right error is `EvalError` with the message Node gives under
`--disallow-code-generation-from-strings`: "Code generation from strings disallowed for this
context". A browser under a Content-Security-Policy answers the same way. That is the error real
libraries already feature-detect on, so it is not an invention.

## Why it is not implemented yet

Adding that branch to `kiln_c99_construct` does not take effect, and I could not find out why
within a reasonable budget. What was ruled out, each checked rather than assumed:

- The branch is inside `kiln_c99_construct` (confirmed by scanning back to the enclosing
  definition), and inside `HEADER`, the prelude constant that spans the emitted C.
- The callee really is that built-in: instrumenting the `New` emitter to record
  `stack[...].bits` printed `method=27`, and `KILN_C99_BUILTIN_FUNCTION` is 27.
- It is not shadowed by an earlier guard: moving the branch to the very first statement of
  `kiln_c99_construct` changed nothing.
- It is not a stale binary: the new message appears in `strings target/release/kiln`, and the
  binary's mtime is newer than the edit.
- It is not the unit cache: the probe was a fresh file with fresh content each time.
- The same function's `new Promise` executor check, added in the same session and sitting a few
  lines *below* the new branch, does take effect. So the function is live and reached.

Those facts are contradictory as stated, which means one of the assumptions behind them is wrong
and worth finding: most likely `new Function` reaches a different construct path than the emitted
`New` instruction appears to use. Start there rather than re-checking the list above.
