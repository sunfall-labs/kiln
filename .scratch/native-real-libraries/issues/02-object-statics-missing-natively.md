# `Object` statics missing or unreachable natively

Status: ready

`Object` has 26 members interpreted. Natively the backend answers 10, then refuses, and because a
refusal aborts, everything alphabetically after the first gap is never reached -- `getPrototypeOf`,
`groupBy`, `hasOwn`, `is` and the rest are hidden behind it. The first gap understates the real
count badly, which is why this is one issue and not one per name.

- `Object.getOwnPropertySymbols` is not implemented at all; the C references it only in comments.
  It refuses a bundled `lodash` at instruction 2793.
- `Object.getOwnPropertyNames` refuses when the receiver is a **builtin constructor**. This is the
  reason the native side cannot enumerate its own surface, which is in turn why the drift went
  unmeasured for so long. It refuses a bundled `vue-reactivity`.

Expect the one-table work to make most of these reachable at once; check this list against it
rather than assuming.
