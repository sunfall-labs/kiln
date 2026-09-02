// What a console shows is not what `String(value)` produces, and the difference is most of the
// value of a console: `String({a:1})` is "[object Object]" and always will be.
console.log({ a: 1, b: 'x', c: true, d: null, e: undefined });
console.log([1, 'two', false, null]);
console.log({}, [], { nested: { deep: [1, 2] } });
console.log('bare string', ['quoted string']);
console.log({ 'needs quotes': 1, valid_$1: 2, 3: 3 });
console.log([[1, [2, [3, [4]]]]]);
console.log(new Set([1, 2]), new Map([['k', 1]]), new Set(), new Map());
console.log(function named() {}, () => {}, Symbol('s'), Symbol.iterator);
console.log({ s: "it's", t: 'a\tb\nc' });
console.log(-0, [-0], 1e21, [1e21]);
// A weak collection shows neither its size nor its contents, because it cannot: an entry is
// reachable only through a key the console does not hold, and enumerating one would make the
// collector's timing observable. Both engines showed `Map(0) {}` for a `WeakMap` -- the right
// count, the wrong container, and a label naming a type the value is not.
const weakKey = {};
const weakMap = new WeakMap([[weakKey, 1]]);
const weakSet = new WeakSet([weakKey]);
console.log(new WeakMap(), new WeakSet());
console.log(weakMap, weakSet);
