# `vue-reactivity` diverged natively: three wrong answers, two fixed, one blocked on a method id

Status: array reactivity fixed; collection reactivity blocked on `Reflect.getPrototypeOf`

The second of the two bundles that ran and printed different bytes.

    node:   array len=3 sum=6|len=4 sum=10
    native: array len=3 sum=6
    node:   collection m=1 size=1 s=1+2|m=2 size=1 s=1+2|m=2 size=1 s=1+2+3
    native: collection m=1 size=1 s=1+2

Both lines were reactive effects that ran once and never re-ran, and each had its own cause. All
three were the backend answering a question it had no storage for, rather than refusing it.

## 1. `Reflect.set` dropped every write to an array -- fixed

`kiln_c99_write_would_accept` probes whether a write will be accepted before performing it, and
read the answer out of the property table. An array's elements and its `length` are the element
store, not entries in that table, and an array nothing has ever named a property on has no table
at all -- so the probe said no for every index and `Reflect.set(a, "3", 4, a)` answered false and
dropped the element. Vue forwards every write through `Reflect.set` inside its set trap, and
spells `push` as `Array.prototype.push.apply(proxy, items)`, so the write vanished and the length
dependency never fired.

The same "no table yet" reading refused every named write to a function, which grows a table on
first use exactly as an array does.

## 2. `Object.isExtensible` said false for a Map -- fixed

A collection carries no extensibility slot, and "no slot" was read as "not extensible". Nothing
had prevented anything; the language says true. Vue's `getTargetType` asks before it will wrap a
collection and reads false as "do not wrap", so `reactive(new Map())` handed back the raw map, no
trap ever ran, and every effect over a Map or a Set was inert.

`preventExtensions` on those kinds is now a refusal: there is nowhere to record it, and one that
quietly did nothing would make the answer above lie in the other direction.

## 3. `in` said false about every collection member -- fixed

A collection is not an object in this backend, so `key in map` fell through the chain walk to the
default and answered false about `get`, `size`, `forEach` and `Symbol.iterator` alike. That is the
other half of the same decision in vue: it installs its tracking instrumentations only for a key
that is `in` the raw target, so a false there silently handed the program the bare `Map` method.

Answered now by the same predicate the read uses, which is also what lets an absent name on a
collection be `undefined` -- `m.nope` and `m.__v_isRef` are what the language answers, not gaps
here -- while a real member the backend has not grown still refuses by name.

## What is left

The bundle's array line now matches node exactly. Its collection line is correct behind the
remaining stop: vue's collection handler does `const { has, get } = Reflect.getPrototypeOf(target)`,
and `Reflect.getPrototypeOf` is in the `builtin_backlog!` table as "not implemented". Measured, by
rewriting only that one arrow to `Object.getPrototypeOf` in a copy of the library: with it the
native backend and the interpreter agree on every collection assertion in the probe, including
`isReactive(reactive(new Map([["k", {}]])).get("k"))`.

It **needs a new method id**. It cannot borrow `Object.getPrototypeOf`'s: that one coerces a
primitive where `Reflect.getPrototypeOf` throws, and the native `Object.getPrototypeOf` refuses
`null` outright where the interpreter throws a TypeError. Reusing it would trade a refusal for a
narrow wrong answer, which is the trade this whole exercise exists to avoid.

Behind that, `console.error` and `console.warn` have no native stderr op, which the probe's last
`[Vue warn]` line reaches. That is an honest refusal already.

## Guarded by

`conformance/native/reactive-collection-shapes.ts` -- the fixed shapes, byte-identical between the
two engines.

`conformance/thirdparty/findings/reflect-set-receiver-argument.js` -- three things this bisect
found and did *not* fix, with the measured output of all three engines:

- `Reflect.set`'s fourth argument is dropped natively, so a write lands on the target where the
  specification lands it on the receiver. Vue never sees it, because its receiver is always a
  proxy over the target whose handler defines neither `getOwnPropertyDescriptor` nor
  `defineProperty`, so the specified path forwards through both traps and ends on the target
  anyway.
- `new Map().constructor` is `Object` in the **interpreter**; node and the native backend both say
  `Map`.
- `Array.prototype.push.apply(proxy, [x])` fires a set trap for every element the **interpreter**
  walked past; node and the native backend write one element and `length`.
- `"clear" in new WeakMap()` is true in the **interpreter**; node and the native backend say false.

The three interpreter defects are recorded there rather than in `conformance/native/` because that
corpus uses the interpreter as its oracle and would read the native backend's correct answer as
the divergence.
