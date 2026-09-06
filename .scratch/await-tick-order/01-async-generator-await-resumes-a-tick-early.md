# An `await` inside an async generator's consumer resumes a tick early, and a thenable is not adopted

**Status:** open

Found 2026-09-06 while proving that the direct await continuation (`kiln_c99_then_continue`)
preserves ordering. It does: the program below prints the same thing on the runtime before and
after that change. What it prints differs from node in two places, and both predate the change.

```js
const log = [];
async function a() {
  log.push("a0"); await 1; log.push("a1"); await Promise.resolve(2); log.push("a2");
  try { await Promise.reject(new Error("boom")); } catch (e) { log.push("a3:" + e.message); }
}
async function* gen() { log.push("g0"); yield await 10; log.push("g1"); yield 21; }
async function c() { for await (const v of gen()) log.push("c:" + v); }
Promise.resolve().then(() => log.push("t0")).then(() => log.push("t1")).then(() => log.push("t2"));
await Promise.all([a(), c()]);
log.push(await { then(res) { res("thenable"); } });
console.log(log.join(" "));
```

| | |
| --- | --- |
| node | `a0 g0 t0 a1 t1 a2 t2 a3:boom c:10 g1 c:21 ... thenable` |
| native | `a0 g0 t0 a1 t1 a2 c:10 g1 t2 a3:boom c:21 ... [object Object]` |

Two defects. The `for await` consumer sees its first value two ticks earlier than node: an async
generator's `yield` awaits its operand, and the consumer's `next()` promise is resolved and then
awaited, and the native driver (`kiln_c99_async_generator_step`) folds those into fewer jobs
than the specification's `AsyncGeneratorResolve` + `Await` sequence. And an awaited non-promise
is queued as a plain value: `await thenable` must call its `then`, and the native driver treats
anything that is not a `KILN_VALUE_PROMISE` as already settled (the same shortcut the old
wrap-in-a-promise path took; `kiln_c99_settle` follows a promise but not a thenable).

Triage: bug, native backend, conformance. Neither is reached by the corpus; the probe
`conformance/native/await-resumes-directly.js` deliberately leaves both out so that it can guard
the ordering the two engines do share.
