// kiln eval smoke test (authored for kiln, not from porffor).
const { klona } = require('klona');
const src = { a: 1, n: [1, { b: 2 }], d: new Date(0), r: /x/g, m: new Map([['k', 'v']]) };
const c = klona(src);
console.log(c !== src, c.n !== src.n, c.n[1] !== src.n[1], c.a === 1);
console.log(c.d.getTime() === 0, c.r.source === 'x', c.m.get('k'));
c.n.push(99);
console.log(src.n.length === 2);
console.log(JSON.stringify(klona([1, 2, 3])));
