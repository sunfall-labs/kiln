const s = Symbol('s');
const src = { a: 1, [s]: 2, [Symbol.iterator]: 3 };
const { a, ...rest } = src;
console.log(({ ...src })[s], rest[Symbol.iterator], Object.keys(src).join(','));
