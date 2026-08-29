const c = { *[Symbol.iterator]() { yield 1; yield 2 } };
console.log(Array.from(c).join(','), [...c].join(','));
