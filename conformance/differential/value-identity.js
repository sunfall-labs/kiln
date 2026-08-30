// Every object kind is one object, and `x === x` has to hold for each of them. A kind left out
// is unequal to itself: a Map keyed on one stops finding it, and a library asking whether a
// value is the one it stored always hears no.
const d = new Date(0);
const r = /x/;
const p = Promise.resolve(1);
console.log(d === d, r === r, p === p);
console.log(d === new Date(0), r === /x/, Promise.resolve(1) === Promise.resolve(1));

const proxied = new Proxy({}, {});
console.log(proxied === proxied, proxied === new Proxy({}, {}));

const bound = function named() {}.bind(null);
console.log(bound === bound, bound === function named() {}.bind(null));

// A BigInt is a primitive: two are equal when the numbers are, however they were built.
console.log(1n === 1n, 1n === 2n, BigInt(5) === 5n, BigInt("5") === 5n);

// The collections and the array searches all rest on the same comparison.
const m = new Map();
m.set(d, "date");
m.set(proxied, "proxy");
console.log(m.get(d), m.get(proxied), m.size, m.has(new Date(0)));
const s = new Set([r, r, d]);
console.log(s.size, s.has(r), s.has(/x/));
console.log([d].indexOf(d), [proxied].includes(proxied), [r].lastIndexOf(r));

// And so does `Object.is`, which differs from `===` only about NaN and the two zeroes.
console.log(Object.is(d, d), Object.is(d, new Date(0)), Object.is(1n, 1n));
