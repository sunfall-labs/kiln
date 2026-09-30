// kiln eval smoke test (authored for kiln, not from porffor).
const traverse = require('json-schema-traverse');
const seen = [];
traverse(
  {type: 'object', properties: {a: {type: 'string'}, b: {items: {type: 'number'}}}},
  {cb: (schema, ptr) => seen.push(ptr)},
);
console.log(seen.join('|'));
