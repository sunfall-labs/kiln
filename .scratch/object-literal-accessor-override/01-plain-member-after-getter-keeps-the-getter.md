# A plain member after a getter of the same name in an object literal keeps the getter

**Status:** open

Found 2026-09-06 while proving the `DefineNamedProperty` site cache with a probe; the divergence
is present on a binary from 2026-09-04, before that cache existed, and the cache never sees this
object (an accessor-bearing target takes the general path).

```js
const lit = { get w() { return 1; }, w: 2, set u(x) {}, u: 3 };
console.log(lit.w, lit.u, JSON.stringify(Object.getOwnPropertyDescriptor(lit, "u")));
```

node: `2 3 {"value":3,"writable":true,"enumerable":true,"configurable":true}`
native: `1 undefined {"enumerable":true,"configurable":true}`

An object literal's member is a *define* (`CreateDataPropertyOrThrow`): it replaces whatever own
property the name had, accessor included. The general path the emitter takes for
`DefineNamedProperty` (`tail.rs`, the `else` branch after `kiln_c99_define_named_fast` declines)
ends in `kiln_c99_put`, which is an assignment: on an accessor it looks for a setter, finds none
for `w`, and calls the setter for `u`, leaving both accessors in place.

Triage: bug, native backend, conformance; low frequency (a literal that spells the same name as
a getter and as a data member). The fix is to route the general branch through a define
(`kiln_c99_define_data_property` or the descriptor path `Object.defineProperty` uses) rather
than `kiln_c99_put`.
