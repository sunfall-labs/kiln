const o = { [Symbol.iterator]() { let i = 0; return { next: () => i < 3 ? { value: i++, done: false } : { done: true } } } };
let s = 0; for (const x of o) s += x;
console.log(s, [...o].join(','));
