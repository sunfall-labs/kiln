# 20. An integer remainder loses negative zero through its integer twin

Status: resolved

Found by issue 18's `native-release` pass. `(value | 0) % 2` for a negative even `value` is `-0`.
`kiln-graph`'s proof (`graph/analysis.rs`, `BinaryOp::Remainder`) calls the result `Proven::Int`
when both operands are ints and the divisor is a non-zero constant, and an `i{v}` twin cannot hold
`-0`. The operation's own box is right (`emit_binary_ints` writes `-0.0` when the result is 0 and
the dividend negative), but every consumer that reads the twin as a double gets `+0`: a typed
entry's `return (double)i{v};` (a `map` callback, as below), phi moves, call arguments.

```js
function constantRemainder(value) { return (value | 0) % 2; }
console.log([-4, -6].map(constantRemainder).map((x) => Object.is(x, -0)));
// interpreter and native-dev: [ true, true ]; native-release: [ false, false ]
```

`conformance/native/numeric-remainder-boxing.js` diverges at `native-release` (pre-existing: the
same with the compiler before issue 18).

## Acceptance

- A remainder is an `Int` only when its dividend is proven non-negative (a non-negative constant,
  `>>>`, `& c` with `c >= 0`, a `length`, a non-negative induction variable), and a `Number`
  otherwise -- or the int twin of a remainder carries its sign some other way. Say which, and
  measure `i % n` loops (`bench/cpu/collatz.mjs`) before and after.
- `numeric-remainder-boxing` matches at `native-release`.

## Comments

2026-09-27, branch `work/release-bugs`.

**Root cause** as filed. `graph/analysis.rs` proved `a % b` an `Int` when both operands were ints and
`b` a non-zero constant, and an `int64_t` twin cannot hold `-0`. The emitter's own box and double
were right, but a typed entry's `return (double)i{v};`, phi moves and call arguments read the
twin.

**Fix** (`backend-c:` commit, `kiln-graph`). A remainder is `Int` only when its dividend is proven
non-negative, and `Number` otherwise, so the box and the double carry the sign. The proof is
`non_negative_int`: a non-negative constant, a non-negative induction variable, `x >>> n`,
`x & c` with a constant `c` in `[0, 2^31)`, a sum of two such values, a remainder of one, or a phi
(or store/TDZ check) of them, walked to depth 8. The emitter's existing non-negative fast form
(`i % k` with no sign fix-up) is unchanged.

**Measured.** `bench/cpu/collatz.mjs` at `native-release`, in-process warm median over three runs
each: base 205/204/195 ms, branch 205/203/196 ms (parity). Collatz's `m % 2` has a `/`-fed
dividend, which was never an int. A non-negative loop (`i % 7`, `(i >>> 0) % 3`, `(i & 0xff) % 5`)
keeps the int path; this is checked in the probe and the unit test.

**Tests.** `kiln-backend-c`: `integer_remainder_is_an_int_only_for_a_non_negative_dividend`
(`(x | 0) % 2` is a number, not an int; `(x >>> 0) % 2` stays an int). `kiln-native`:
`release_integer_remainder_of_a_negative_dividend_is_negative_zero`. Probe
`conformance/native/release-integer-remainder-negative-zero.js` covers the typed-entry return
(`map`), a phi, a call argument with a negative divisor, a loop comparison, and the non-negative
int path. On base it diverges (`[false, false, ...]`, `Infinity` for `1 / -0`); on the branch it
matches. `numeric-remainder-boxing` matches at `native-release`.

**The release corpus pass** (every `conformance/native` probe forced to `native-release`, a copy
of `native.sh` with `timeout -k 10 900` per native run and no stop at the first broken build, on
the branch's final compiler): **279 matched, 0 diverged, 1 refused** -- `shadow-realm`, which
refuses in every profile. `scripts/native.sh`: 279 matched, 1 refused (`shadow-realm`), 0 diverged.
