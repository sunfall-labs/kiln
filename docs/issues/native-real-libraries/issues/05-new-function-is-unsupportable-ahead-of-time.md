# `new Function` blocks `zod` and `ajv`, and always will

Status: done -- ADR 0045

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

## Resolved

Implemented as ADR 0045. `zod` compiles and runs natively and matches node byte for byte; `ajv`
gets past this wall to a much smaller one (`console.error`).

The earlier note here said the change "does not take effect" and listed six eliminations. That was
wrong, and the way it was wrong is worth keeping. The branch raising the error was correct from the
first attempt. `kiln_c99_throw_error` could not *build* an `EvalError`, because
`kiln_c99_error_name` -- the function that decides whether a built-in is an error -- listed six of
the language's seven and omitted it. A throw that cannot be built returns a plain error status, so
the caller reported what it was doing at the time, which read exactly like the branch never running.

Every one of the six eliminations was individually true and the conclusion drawn from them was
false: they all asked "does my code run?" and none asked "does the thing my code calls work?".
Swapping the error kind for `TypeError` answered it in one build.
