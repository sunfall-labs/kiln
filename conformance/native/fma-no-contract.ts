// Under IEEE 754 without FMA: (1 + eps) * (1 - eps) = 1 - eps^2, which rounds to 1.0 in float64.
// 1.0 - 1.0 = 0.0.
// With FMA contraction, (1 + eps) * (1 - eps) - 1.0 computes -eps^2 directly (-4.930380657631324e-32).
const a = 1.0 + Number.EPSILON;
const b = 1.0 - Number.EPSILON;
const c = -1.0;
const result = a * b + c;
console.log(result === 0 ? "pass: no fma contraction" : `fail: ${result}`);
