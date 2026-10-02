# PRD: Generator.throw() Resumes Catch Handler in Suspended Body

Status: resolved
Branch: `fix/generator-throw-resumes-body`

## Problem Statement

Calling `generator.throw(x)` on a started, suspended generator never entered the generator body, preventing a `try/catch` wrapping the `yield` expression from handling the exception.

## Resolution

- In native runtime: `KILN_C99_METHOD_GENERATOR_THROW` now re-enters a started, suspended generator body covered by a handler with `raising` set and the value in `sent`.
- In IR/codegen: graph emitter writes `frame->handler_top` on yield suspension so the handler stack is preserved across turns.
