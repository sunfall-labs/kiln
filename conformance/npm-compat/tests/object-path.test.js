// kiln eval smoke test (authored for kiln, not from porffor).
const objectPath = require('object-path');
const o = {a: {b: 2}};
console.log(objectPath.get(o, 'a.b'));
console.log(objectPath.has(o, 'a.b'));
console.log(objectPath.get(o, 'x.y', 'dflt'));
objectPath.set(o, 'a.c', 3);
console.log(JSON.stringify(o));
objectPath.del(o, 'a.b');
console.log(JSON.stringify(o));
console.log(objectPath.get({l: [{m: 7}]}, 'l.0.m'));
console.log(objectPath.has(o, 'a.c'));
