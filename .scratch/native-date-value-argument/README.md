# `new Date(object)` never asks the object what it is, in the native backend only

**Status:** open. Native backend. A **silent wrong answer**, not a refusal.

## What is wrong

```ts
let seen = "none";
const spy: any = { [Symbol.toPrimitive]: function (h: string) { seen = h; return 5; } };
const d = new Date(spy);
console.log(seen, d.getTime());
// node:        default 5
// interpreter: default 5
// native:      none NaN
```

The native `Date` constructor reads its single argument with a pure numeric conversion. An object
has no number, so it answers NaN, and NaN is a *valid* instant to store -- it is what `new
Date("nonsense")` produces. The program gets an Invalid Date rather than a stop, and the mistake
surfaces later as a formatted `"Invalid Date"` string somewhere else entirely.

This is the class the backend otherwise refuses: nothing throws, the read and the write agree, and
the cause is several frames upstream of the symptom. It is not caught by `scripts/native.sh`
because no probe in `conformance/native` constructs a date from an object.

## What this is *not*

Not the hint bug fixed alongside this filing. That one was in both engines and was about *which*
hint `ToPrimitive` was asked for; this one is about the native `Date` constructor not calling
`ToPrimitive` at all. `spy + 1` reduces correctly in the native backend today, with the "default"
hint -- so the machinery exists and the constructor simply does not use it.

## The shape a fix takes

`kiln_c99_to_primitive` is already reachable and already answers "default" correctly. The date
constructor needs to route a single non-string, non-date argument through it before converting,
which is what the interpreter's `Native::DateConstructor` arm does. The arity matters: one argument
is `ToPrimitive` with no preferred type, and every other arity is the field form, which is seven
numbers converted left to right in an order a `valueOf` can observe.

Worth pairing with a `conformance/native` probe, since the absence of one is why this survived.
