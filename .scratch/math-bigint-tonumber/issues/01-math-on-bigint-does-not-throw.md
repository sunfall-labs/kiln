# `Math.sqrt(10n)` answers a number instead of throwing

Found 2026-09-06 while proving that a `Math` intrinsic's answer is a number. Both engines agree
with each other and disagree with node, so this is in the shared `Math` implementation, not in
either backend.

```js
for (const [n, f] of [["sqrt", () => Math.sqrt(10n)], ["floor", () => Math.floor(1n)],
                      ["max", () => Math.max(1n, 2)], ["abs", () => Math.abs(-1n)]]) {
  try { console.log(n, f()); } catch (e) { console.log(n, "throws", e.constructor.name); }
}
```

node throws `TypeError` from every one: `Math.*` applies `ToNumber`, and `ToNumber` of a BigInt
throws (ECMA-262 §7.1.4). kiln, interpreted and native alike, answers `3.1622776601683795`,
`1`, `2`, `1`: the BigInt is converted as if it were a Number.

The native backend's plan treats a `Math` intrinsic's answer as a number, which stays right
either way -- the call answers a number or throws -- so nothing there depends on this fix.

Triage: bug, language semantics, both engines. The fix is one `ToNumber` that refuses a BigInt,
wherever the shared `Math` implementation converts its argument.

Status: resolved · Severity: medium
- In native C backend: `kiln_c99_to_number_checked` now explicitly checks for and rejects `KILN_VALUE_SYMBOL` and `KILN_VALUE_BIGINT` with `TypeError: Cannot convert a Symbol/BigInt value to a number`. `KILN_C99_BUILTIN_NUMBER` handles BigInt conversion via `ToNumeric`. `MathNumber` group added to Math methods in `crates/native/kiln-backend-c/src/prelude.rs` so argument conversion goes through `kiln_c99_to_number_checked`.
- In interpreter: `dispatch_native` validates arguments left-to-right rejecting `BigInt` (when hint is `PrimitiveHint::Number` and native != `NumberConstructor`) and `Symbol` with `TypeError`. `argument_number` and `argument_number_at` return `Result<f64, ExecutionError>`.
- Verified across interpreter and native with `conformance/regress/r13-math-bigint-tonumber.js` and `conformance/native/math-bigint-tonumber.ts`.
