// `Array.from` walks the iterator protocol, falls back to array-like indexing, and applies the
// optional mapping function -- none of which a pure argument-to-value builtin can do.
const iterable = { *[Symbol.iterator]() { yield 'a'; yield 'b'; } };
console.log(Array.from(iterable).join(','));
console.log(Array.from(iterable, (value, index) => value + index).join(','));
console.log(Array.from([1, 2, 3], (n) => n * 2).join(','));
console.log(Array.from('hi').join(','));
console.log(Array.from({ length: 2 }).length, String(Array.from({ length: 2 })[0]));
console.log(Array.from(new Set([1, 1, 2])).join(','));
