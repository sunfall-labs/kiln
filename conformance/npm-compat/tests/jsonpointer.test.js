// kiln eval smoke test (authored for kiln, not from porffor).
const jsonpointer = require('jsonpointer');
const obj = { a: { b: 2 }, list: ['x', 'y'] };
console.log(jsonpointer.get(obj, '/a/b'));
console.log(jsonpointer.get(obj, '/list/1'));
jsonpointer.set(obj, '/a/c', 3);
console.log(JSON.stringify(obj.a));
const compiled = jsonpointer.compile('/a/b');
console.log(compiled.get(obj));
console.log(jsonpointer.get(obj, '') === obj);
