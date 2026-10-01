// kiln eval smoke test (authored for kiln, not from porffor).
import { flatten, unflatten } from 'flat';
console.log(JSON.stringify(flatten({ a: { b: 1 }, c: [2, 3] })));
console.log(JSON.stringify(unflatten({ 'a.b': 1 })));
console.log(JSON.stringify(flatten({ a: { b: { c: 'deep' } } })));
console.log(JSON.stringify(unflatten(flatten({ x: [1, { y: 2 }] }))));
