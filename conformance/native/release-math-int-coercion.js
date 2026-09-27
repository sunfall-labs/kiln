// kiln-native: profile=native-release
//
// `Math.clz32` and `Math.imul` take `ToUint32`/`ToInt32` of their arguments. The release
// intrinsics converted a non-number with the pure runtime conversion (an object's `valueOf` was
// never called) and cast a double to `int64_t` directly, which is undefined for NaN and the
// infinities (.scratch/parallel-builtins issue 18).
let calls = 0;
const three = { valueOf() { calls++; return 3; } };
function f(a, b) { return [Math.clz32(a), Math.imul(a, b)]; }
console.log(f(three, three), calls);
console.log(f(1, 2), f(NaN, Infinity), f(-Infinity, 5), f(-1, 2 ** 40 + 3), f(2 ** 32 + 5, -7.9), f("8", 2), f(-0, -0));
function g(n) { let s = 0; for (let i = 0; i < n; i++) s = (s + (Math.imul(i, 0x9e3779b1) ^ Math.clz32(i))) | 0; return s; }
console.log(g(1000));
