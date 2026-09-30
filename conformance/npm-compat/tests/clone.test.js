// kiln eval smoke test (authored for kiln, not from porffor).
const clone = require('clone');
const a = { x: 1, nested: { y: [1, 2] } };
const b = clone(a);
b.nested.y.push(3);
console.log(JSON.stringify(a.nested.y));
console.log(JSON.stringify(b.nested.y));
const arr = clone([{ p: 1 }, { p: 2 }]);
arr[0].p = 99;
console.log(JSON.stringify(arr.map(o => o.p)));
const d = clone({ when: new Date('2024-01-01T00:00:00Z') });
console.log(d.when.toISOString());
console.log(clone('str'), clone(42), clone(null));
