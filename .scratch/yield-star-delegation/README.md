# `yield*` does not delegate `return` or `throw` to the inner iterator

**Status:** open. ~53 test262 rows (27 files x 2 modes): 13 `star-rhs-iter-rtrn-*`,
9 `star-rhs-iter-thrw-*`, 5 others, all in `language/expressions/yield`.

## What is missing

`yield_delegate` (`crates/kiln-ir/src/compile.rs:3257`) emits a loop that calls the inner
iterator's `next` and nothing else. The specification's `yield*` is a three-way protocol: when the
*outer* generator is resumed with a return or a throw completion, that completion is forwarded to
the inner iterator's `return` or `throw`.

Observed against node:

```js
var inner = { [Symbol.iterator]() { return {
  next() { return { value: 1, done: false }; },
  return() { return 5; },            // not an object
}; } };
function* g() { yield* inner; }
var it = g(); it.next();
it.return(42);
// node: TypeError (the inner result is not an object)
// kiln: {value: 42, done: true}  -- the inner `return` result is ignored
```

and, with an inner iterator that has no `throw`:

```js
it.throw(new Error("x"));
// node: closes the inner iterator, then throws TypeError
// kiln: propagates the original Error, and the inner `return` is never called
```

Three distinct defects, all from the same absence: a non-object result from the inner `return`
is not refused, the inner result is discarded rather than propagated, and a missing inner `throw`
neither closes the inner iterator nor raises the TypeError the specification requires.

## Why this can be emitted IR

Worth writing down, because it decides the shape of the fix. The VM already raises *both*
resumption kinds as throwable values at the suspension point
(`crates/kiln-interpreter/src/lib.rs`, around the `raising` branch):

- a return completion is raised as `Value::Native(Native::ReturnSignal)`;
- a throw completion is raised as the thrown value itself.

So a handler emitted around the `Yield` inside the delegate loop can observe both, which means
the whole protocol can live in `yield_delegate` as emitted instructions -- no new instruction, and
both engines get it at once, the way the iterator-result guard and the derived-constructor return
guard did.

Two things to work out before writing it:

- **Telling the two apart.** The loop must distinguish the `ReturnSignal` from an ordinary thrown
  value. There is no IR-visible test for it today; the cheapest honest option is probably a
  dedicated instruction or a comparison against a value the prelude and the interpreter both
  name, and whichever is chosen has to exist in both engines.
- **Re-raising.** After the inner iterator has been given its `return`, a completion that is
  still a return has to go on unwinding the outer generator. Swallowing it would turn
  `it.return()` into an ordinary resumption.

## Why it was not done in the same pass as the other guards

Blast radius. This is inside generator unwinding, which `language/statements/generators`,
`language/expressions/async-generator` and every `for-of` over a generator depend on -- several
thousand passing rows. The two guards it resembles were leaf-position and could not affect a
program that did not hit them; this one changes how every `yield*` is resumed.
