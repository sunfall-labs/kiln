// `Math` and `JSON` and `Reflect` are namespace *objects*, not functions: they cannot be called,
// and a library that feature-detects with `typeof Math === "object"` has to find one. kiln
// reported "function" for all three, which is the kind of answer that sends a bundle down a
// polyfill path nobody tested.
console.log(typeof Math, typeof JSON, typeof Reflect, typeof Object, typeof Array, typeof String);
console.log(typeof Math, typeof Object, typeof NaN, typeof Infinity, NaN, Infinity, -Infinity);
console.log(Math.PI, Math.E, Math.floor(1.7), Math.ceil(1.2), Math.round(1.5), Math.abs(-3));
console.log(Math.max(1,5,3), Math.min(1,5,3), Math.max(), Math.min(), Math.sqrt(9), Math.pow(2,10));
console.log(Math.trunc(-1.7), Math.sign(-5), Math.log(1), Math.exp(0), Math.hypot(3,4), Math.cbrt(27));
console.log(Number.isInteger(5), Number.isFinite(1/0), Number.isNaN(NaN), Number.parseFloat("1.5"));
console.log(Number.MAX_SAFE_INTEGER, Number.EPSILON, Number.POSITIVE_INFINITY, Number.MIN_SAFE_INTEGER);
const o = {b: 2, a: 1};
console.log(Object.keys(o), Object.values(o), Object.entries(o), Object.hasOwn(o, "a"));
console.log(Object.assign({}, o, {c: 3}), Object.freeze(o) === o, Object.isFrozen(o));
console.log(Array.isArray([]), Array.isArray({}), Array.of(1,2));

console.log(Math.sin(0), Math.cos(0), Math.tan(0), Math.atan2(1, 1), Math.asin(0), Math.acos(1));
console.log(Math.hypot(), Math.hypot(3, 4), Math.hypot(1e300, 1e300) === Infinity);
console.log(Math.LN2, Math.LN10, Math.SQRT2, Math.SQRT1_2, Math.LOG2E, Math.LOG10E);
console.log(Math.log2(8), Math.log10(1000), Math.cbrt(27), Math.sqrt(2));
