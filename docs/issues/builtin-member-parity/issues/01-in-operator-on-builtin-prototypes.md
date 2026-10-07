# Issue 01: `in` on a built-in prototype was wrong on both engines, in opposite directions

Status: resolved
Feature: builtin-member-parity
Governing ADR: ADR 0054

Split out of `docs/issues/array-prototype-parity/issues/01`, which found it while describing
`parallelMap` and resolved without it.

## What

Node answers `true` for a member and `false` for a name no engine has. Neither engine did.

```js
console.log("map" in Array.prototype, "nope" in Array.prototype);
// node:        true  false
// interpreter: true  true      <- true for every string, including invented ones
// native:      false false     <- false for every member it demonstrably has
```

Native's answer is the expensive one CLAUDE.md names: a feature test takes the branch written for
a realm that never had the member. It is what made `@kiln/concurrency` install a polyfill over
`parallelMap` on the one backend that had just grown it (ca2de630), and the polyfill then hit a
refusal on `defineProperty`. The interpreter's answer is worse for detection in the other
direction: `"nope" in Object.prototype` was true.

The interpreter was wrong only for `Native::PrototypeOf(table)`; `Math`, `JSON` and the
constructors already answered correctly.

## Resolution (2026-09-07)

**Interpreter.** `has_property` asked `Native::property(name).is_some()`. That is what a *read*
finds, and a prototype table's `property` both carries its last step up to `Object.prototype` and
answers `Some(Value::Undefined)` for a name it does not have -- so `is_some()` was true for every
string. It now asks `intrinsic_owns`, the ownership test `hasOwnProperty` and
`getOwnPropertyNames` already agree on, whose own comment describes this exact trap. The step up
to `Object.prototype` is taken separately below, so `"hasOwnProperty" in Array.prototype` stays
true.

**Native.** `kiln_c99_has_property` had no `KILN_VALUE_BUILTIN` arm at all, so a built-in fell
through to the object chain walk, which stops at the first value that is not an object. Added
`kiln_c99_builtin_has_member`, which answers out of the membership predicates rather than by
attempting a read -- the same split the regexp arm already makes, and for the same reason: a read
refuses a member of the language this backend has not built, which is right for
`String.prototype.anchor` and wrong for `"anchor" in String.prototype`.

`conformance/native/builtin-member-in.ts` and `conformance/differential/builtin-member-in.js` pin
it against node across `Array`, `String`, `Object`, `Number`, `Function`, `Promise`, `Map`, `Set`,
`Date` and `RegExp` prototypes, their instances, `Math`, `JSON` and the constructors.
