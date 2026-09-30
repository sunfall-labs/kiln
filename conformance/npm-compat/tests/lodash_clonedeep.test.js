// kiln eval smoke test (authored for kiln, not from porffor).
const cloneDeep = require('lodash.clonedeep');

const src = { a: 1, b: { c: [1, 2, { d: 3 }] }, e: new Date('2020-01-01T00:00:00Z') };
const copy = cloneDeep(src);
copy.b.c[2].d = 99;
console.log('deep independence:', src.b.c[2].d === 3, copy.b.c[2].d === 99);
console.log('date cloned:', copy.e instanceof Date, copy.e.getTime() === src.e.getTime());

const circ = { x: 1 };
circ.self = circ;
const circCopy = cloneDeep(circ);
console.log('circular:', circCopy.self === circCopy, circCopy !== circ);

console.log('array:', JSON.stringify(cloneDeep([1, [2, [3]]])));
console.log('map:', cloneDeep(new Map([['k', { v: 1 }]])).get('k').v === 1);
console.log('primitives:', cloneDeep(42) === 42, cloneDeep('s') === 's', cloneDeep(null) === null);
