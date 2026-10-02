# 19. Node-parity gaps found by the messaging work

Status: resolved

Found while finishing issue 14; each small, each a divergence from node:

1. Natively, `BigInt64Array` and `BigUint64Array` elements read as `NaN` in every profile.
2. `setTimeout(fn, 0)` is not clamped to 1 ms as node clamps it, which changes ordering against
   `setImmediate` in node's timers phase.
3. `Event.prototype` lacks node's `util.inspect.custom` hook, so `console.log(event)` differs.

## Acceptance

- A `conformance/native` or `conformance/regress` probe per item, matching node; `scripts/native.sh`
  0 diverged.

## Comments

**2026-09-27 — branch `work/node-parity-rejections`: resolved.**

1. **BigInt64Array/BigUint64Array natively.** `kiln_c99_typed_element` answers a BigInt for the
   two BigInt kinds (the eight bytes through `kiln_c99_u64_bigint`) and a Number otherwise, and
   `kiln_c99_typed_store` writes one through `ToBigInt` and the low sixty-four bits (the
   `setBigInt64` conversion, now `kiln_c99_to_bigint64_bits`, shared with DataView). Every
   element path reads through them: an index, iteration, spread, `Object.entries/values`,
   join/String, inspection, JSON, the callback methods, `set`, `from` and construction from an
   array or another view (a Number view from a BigInt one, or the reverse, is the TypeError).
   Probe: `conformance/native/bigint64-array-elements.js`, identical to node in both engines and
   the release profile.
2. **Timer delays.** `kiln_runtime::Realm::node_timer_delay` is node's rule, used by both engines
   (the interpreter directly, the native provider's `arm`, so no C-side clamp remains): anything
   but 1 to 2^31 - 1 ms is 1 ms, and a NaN, a negative or an overflowing delay writes node's
   `TimeoutNaNWarning` / `TimeoutNegativeWarning` / `TimeoutOverflowWarning` to stderr with the
   `--trace-warnings` hint once per realm. `process.pid` is now the real pid in both engines
   (it was absent in the interpreter and 1 natively), which the warning names. Probes:
   `conformance/regress/r25-timer-delay-clamp` (node's order, including the warning cases) and
   `conformance/native/timer-delay-clamp.js`; `conformance/native/timers.ts` no longer arms a
   negative delay, since the warning's pid differs between the two processes native.sh compares.
3. **`Event.prototype[util.inspect.custom]`** (TypeScript, `web_events_streams.ts`): the class
   name and node's four fields with node's string quoting and its 80-column one-line rule. For
   `console.log(event)` to use it, both engines' console now honours `util.inspect.custom` at the
   top level (the interpreter's `console_arguments`, the native `kiln_c99_console_inspect` on the
   console host calls): the hook gets `(2, {}, undefined)`, a string answer prints as it stands,
   anything else is inspected, and a hook that throws (one that needs the `inspect` argument)
   leaves the value to the ordinary formatting. Probes: `conformance/regress/r26-event-inspect-custom`,
   `conformance/native/event-inspect-custom.cjs`.

Found, not fixed: natively a class method with a symbol key is enumerable
(`Object.getOwnPropertyDescriptor(C.prototype, sym).enumerable` is `true`; the interpreter and
node say `false`) -- `kiln_c99_define_shaped` records a reserved key without the enumerable flip,
and the descriptor read disagrees; that code is shared with the object-model work. Nested custom
inspection (an event inside an array) is still the engines' plain formatting.
