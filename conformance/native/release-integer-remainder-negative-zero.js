// kiln-native: profile=native-release
//
// A remainder takes the sign of its dividend, so `-4 % 2` is `-0`. The graph proof used to call
// an integer remainder by a non-zero constant an `Int` whatever its dividend, and an `int64_t`
// twin cannot hold `-0`: every consumer that read the twin as a double -- a typed entry's
// return, a phi, a call argument -- saw `+0` (issue 20). A remainder is an `Int` now only when
// its dividend is proven non-negative.

const isNegZero = (x) => Object.is(x, -0);

// Through a typed entry's return (the `map` callback).
function constantRemainder(value) {
  return (value | 0) % 2;
}
console.log([-4, -6, 4, -3, 3].map(constantRemainder).map(isNegZero));
console.log([-4, -6, 4, -3, 3].map(constantRemainder).join());

// Through a phi.
function viaPhi(value, flag) {
  let r = 1;
  if (flag) r = (value | 0) % 3;
  return r;
}
console.log(isNegZero(viaPhi(-9, true)), isNegZero(viaPhi(9, true)), viaPhi(-7, true));

// Through a call argument, and a negative divisor (the sign is still the dividend's).
function take(x) {
  return 1 / x;
}
function viaArgument(value) {
  return take((value | 0) % -5);
}
console.log(viaArgument(-10), viaArgument(10), viaArgument(-7));

// Through a loop-carried sum and a comparison: `-0` and `+0` compare equal.
function loop(values) {
  let zeros = 0;
  let negatives = 0;
  for (let i = 0; i < values.length; i++) {
    const r = (values[i] | 0) % 4;
    if (r === 0) zeros++;
    if (isNegZero(r)) negatives++;
  }
  return [zeros, negatives];
}
console.log(loop([-8, -4, 0, 4, 8, -3, 5]));

// A non-negative dividend keeps the integer path and its answers.
function nonNegative(n) {
  let acc = 0;
  for (let i = 0; i < n; i++) {
    acc += i % 7;
    acc += (i >>> 0) % 3;
    acc += (i & 0xff) % 5;
  }
  return acc;
}
console.log(nonNegative(1000), isNegZero((0 >>> 0) % 2));
