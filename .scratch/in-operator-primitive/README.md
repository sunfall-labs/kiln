# `in` does not refuse a primitive right-hand side

**Status:** FIXED 2026-09-03, on the second attempt. +4 rows
(`language/expressions` 20360 -> 20364). The first attempt is kept below because the mistake is
the useful part.

The predicate the C backend wanted was `kiln_c99_is_reference(v) || v.tag == KILN_VALUE_DATE`.
`is_reference` is a *denylist* of primitives and its own comment says a Date is deliberately not
one -- "`as_number` already gives its instant, which is what `valueOf` would produce" -- a
shortcut for date arithmetic that makes it the wrong predicate for any other question. Reading
that comment was what fixed it; guessing from the name is what broke it.

**Original filing:**

```js
"x" in 1      // TypeError in every other engine; kiln answers false
"x" in null   // TypeError; kiln answers false
```

`in` asks an *object* what it has, and the specification refuses anything else. Answering `false`
tells a program the property is absent from something that could never have had it -- the same
wrong answer for a typo'd receiver as for a deliberate check.

## What went wrong when I tried it

The interpreter side was fine: `is_an_object(right) || is_callable(right)` accepted arrays, maps,
typed arrays, proxies, dates, regexps, wrappers and promises, all verified against node.

The C backend was not. I reached for `kiln_c99_is_reference`, which is the predicate
`ToPrimitive` uses, and it is **narrower than "is an object"** -- `'a' in new Date()` became a
TypeError. `scripts/thirdparty.sh` caught it immediately: `thirdparty (native): 18 matched,
1 diverged`, from 19/0. Reverted in both engines; the gate confirmed 19/0 restored.

So the fix needs the C backend's *own* notion of "is an object", not the ToPrimitive one. Worth
finding out whether one exists before writing another -- and worth checking what
`kiln_c99_is_reference` is actually for, because its name suggests a generality it does not have.

## Also noticed while probing -- since fixed

`'source' in /x/` answered `true` in the interpreter and `false` natively, and so did `flags`,
`lastIndex` and `exec`. `kiln_c99_has_property` has arms for ArrayBuffer, DataView, TypedArray
and Array and had none for RegExp, so the walk fell through to a generic chain that does not know
a regexp's prototype. `"exec" in rx` is how a program feature-detects one.

Fixed by listing the accessors and the prototype's members. Two things that did not work on the
way: a regexp has no property store natively -- `kiln_regexp` has no field for one -- and probing
by *reading* the member refuses for a name the backend has not implemented, which is right for
`rx.nope` and wrong for `"nope" in rx`, where the question is answerable and the answer is false.
