// FOUND BY `differential_native`, seed 101, case 107. Reduced by hand from a 25-line generated
// program to this.
//
// The native backend answers NaN where the interpreter and node answer a number. Both engines run
// to completion, both exit 0, and nothing is printed on stderr: a silent wrong answer, which
// `scripts/native.sh` calls "the one outcome this check exists to stop".
//
// The pattern: ToNumber applied to an ARRAY is correct on the operator paths and wrong on the
// builtin-call paths. `+[7]` and `[8] * 1` convert; `Number([6])`, `Math.abs([2])`,
// `Math.floor([4])` and `Math.trunc([5])` do not. Primitives are converted correctly everywhere,
// so it is specifically ToPrimitive that the builtin path is missing.
//
//   node        0 2 3 0 4 5 6 7 8 NaN
//   interpreter 0 2 3 0 4 5 6 7 8 NaN
//   native      NaN NaN 3 NaN NaN NaN NaN 7 8 NaN
//
// Verify with:  cd fuzz && cargo run --release --bin differ -- findings/tonumber-on-an-array.js
console.log(Math.abs([0]));
console.log(Math.abs([2]));
console.log(Math.abs("3"));
console.log(Math.abs([]));
console.log(Math.floor([4]));
console.log(Math.trunc([5]));
console.log(Number([6]));
console.log(+[7]);
console.log([8] * 1);
console.log(Math.abs({}));
