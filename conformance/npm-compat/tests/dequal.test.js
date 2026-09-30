// kiln eval smoke test (authored for kiln, not from porffor).
const { dequal } = require('dequal');
console.log(dequal({ a: 1 }, { a: 1 }));
console.log(dequal([1, [2]], [1, [2]]));
console.log(dequal({ a: 1 }, { a: 2 }));
console.log(dequal(new Map([['k', 'v']]), new Map([['k', 'v']])));
console.log(dequal(/abc/gi, /abc/gi));
