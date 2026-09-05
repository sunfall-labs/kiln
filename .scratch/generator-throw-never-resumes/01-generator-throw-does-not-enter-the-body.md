# `generator.throw(x)` never resumes the body, so a `try` around the `yield` cannot catch it

**Status:** closed / resolved 2026-09-05. `KILN_C99_METHOD_GENERATOR_THROW` now re-enters a
started, suspended body whose suspension a handler covers, with `raising` set and the value in
`sent`, and builds the step from what the body yields or returns; `next` clears `raising` so a
body that caught once is not thrown into again. The graph emitter writes `frame->handler_top` at
every suspend (whether a handler covers it), which is what `return()` and `throw()` read.
Probe: `p.throw("x").value, p.next().value` prints `caught:x fin`, as node does.

Found 2026-09-05 while probing the graph emitter's generator lowering. The interpreter is right;
the native runtime is not, on the pre-graph binary and on the graph emitter alike, so this is in
the prelude, not in either emitter.

```js
function* pairs(a, b) {
  yield a + b;
  try { yield a * b; } catch (e) { yield "caught:" + e; } finally { yield "fin"; }
}
const p = pairs(3, 4);
p.next(); p.next();
console.log(p.throw("x").value, p.next().value);   // node: caught:x fin
```

`KILN_C99_METHOD_GENERATOR_THROW` in `prelude_core.c` marks the frame done and returns the
thrown value to the caller without re-entering the body. The `return` method already has the
mechanism -- `frame->raising = 1; frame->sent = ...;` then `kiln_c99_invoke_closure_body` -- and
the graph emitter's resume blocks honour `raising` by throwing into the block's handler, so the
fix is the throw method doing what the return method does when a handler is active, and
throwing to the caller only when the body propagates it.

The `return` path decides whether to re-enter by `frame->handler_top != 0`, the frame's dynamic
handler stack, which the graph emitter does not maintain; a static emitter can set
`frame->handler_top` at each suspend to the block's handler depth, and should, once this is fixed.

Triage: bug, native runtime, generators. Probe: the `pairs` generator above.
