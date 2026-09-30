// kiln eval smoke test (authored for kiln, not from porffor).
const jsonpatch = require('fast-json-patch');
const doc = {a: 1, b: [1, 2]};
console.log(JSON.stringify(jsonpatch.applyPatch(doc, [{op: 'replace', path: '/a', value: 9}]).newDocument));
console.log(JSON.stringify(jsonpatch.compare({a: 1}, {a: 2})));
console.log(JSON.stringify(jsonpatch.compare({x: 1}, {})));
const d2 = {n: 'orig'};
jsonpatch.applyPatch(d2, [{op: 'add', path: '/m', value: [1]}]);
console.log(JSON.stringify(d2));
console.log(JSON.stringify(jsonpatch.compare([1, 2], [1, 3])));
