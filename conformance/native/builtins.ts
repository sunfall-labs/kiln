// The builtin namespaces the backend supplies. `typeof` reports what each one is: a namespace is
// an object holding functions and cannot be called, where a constructor is a function.
console.log(typeof Math, typeof Object, typeof Number, typeof Array, typeof String);
console.log(NaN, Infinity, -Infinity, typeof NaN, NaN === NaN, Infinity > 1e308);

console.log(Math.PI, Math.E, Math.LN2, Math.LN10, Math.SQRT2, Math.SQRT1_2, Math.LOG2E, Math.LOG10E);
console.log(Math.floor(1.7), Math.ceil(1.2), Math.round(1.5), Math.round(-0.5), Math.trunc(-1.7));
console.log(Math.abs(-3), Math.sign(-5), Math.sign(0), Math.sqrt(9), Math.cbrt(27), Math.pow(2, 10));
console.log(Math.min(1, 5, 3), Math.max(1, 5, 3), Math.min(), Math.max(), Math.min(1, NaN));
console.log(Math.log(1), Math.log2(8), Math.log10(1000), Math.exp(0), Math.hypot(3, 4), Math.hypot());
console.log(Math.sin(0), Math.cos(0), Math.tan(0), Math.atan2(1, 1), Math.asin(0), Math.acos(1), Math.atan(0));

// `Number.isNaN` does not coerce where the global `isNaN` does, so a string is simply not a NaN.
console.log(Number.isInteger(5), Number.isInteger(5.5), Number.isInteger("5"), Number.isNaN(NaN));
console.log(Number.isNaN("x"), Number.isFinite(1 / 0), Number.isFinite(1), Number.isSafeInteger(2 ** 53));
console.log(Number.MAX_SAFE_INTEGER, Number.MIN_SAFE_INTEGER, Number.EPSILON);
console.log(Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY, Number.NaN);

const o = { b: 2, a: 1 };
console.log(Object.keys(o), Object.values(o), Object.entries(o));
console.log(Object.hasOwn(o, "a"), Object.hasOwn(o, "z"), Object.keys({}), Object.entries({}));
console.log(Object.assign({}, o, { c: 3 }), Object.assign({ x: 0 }, { x: 1 }));

console.log(Array.isArray([]), Array.isArray({}), Array.isArray("x"), Array.of(1, 2), Array.of());

// A namespace method is a value, and the same one twice is the same function.
console.log(Math.floor === Math.floor, typeof Math.floor, Math.floor, Object.keys, Number.isNaN);

// Reached through an expression and used as a callback, which is how they are written.
const sizes = [1.2, 3.7, 2.5];
console.log(sizes.map((n: number) => Math.floor(n)), sizes.map(Math.floor));
console.log(Object.keys(o).map((k: string) => k.toUpperCase()).join(","));
