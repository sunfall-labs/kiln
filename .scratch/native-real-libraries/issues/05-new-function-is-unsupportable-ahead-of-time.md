# `new Function` blocks `zod` and `ajv`, and always will

Status: needs a decision

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
