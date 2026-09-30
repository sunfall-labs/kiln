// kiln eval smoke test (authored for kiln, not from porffor).
const defu = require('defu');
console.log(JSON.stringify(defu({ a: 1 }, { a: 9, b: 2 })));
console.log(JSON.stringify(defu({ nested: { x: 1 } }, { nested: { y: 2 } })));
console.log(JSON.stringify(defu({}, { a: 1 })));
console.log(JSON.stringify(defu({ a: [1] }, { a: [2], b: 3 })));
