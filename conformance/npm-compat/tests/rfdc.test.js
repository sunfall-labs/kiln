// kiln eval smoke test (authored for kiln, not from porffor).
const clone = require('rfdc')();

const original = { a: 1, nested: { arr: [1, 2, { deep: true }] }, d: new Date(0) };
const copy = clone(original);

copy.nested.arr[2].deep = false;
copy.a = 999;
console.log(original.a, original.nested.arr[2].deep);
console.log(JSON.stringify(copy.nested.arr));

const protoClone = require('rfdc')({ proto: true });
class Foo { bar() { return 'baz'; } }
const inst = new Foo();
const instCopy = protoClone(inst);
console.log(typeof instCopy.bar, instCopy instanceof Foo);
