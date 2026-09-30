// kiln eval smoke test (authored for kiln, not from porffor).
const get = require('lodash.get');
const obj = { a: { b: { c: 3 } }, list: [{ x: 1 }] };
console.log(get(obj, 'a.b.c'));
console.log(get(obj, 'a.x.y', 'default'));
console.log(get(obj, ['a', 'b', 'c']));
console.log(get(obj, 'list[0].x'));
console.log(get(null, 'a.b', 'safe'));
