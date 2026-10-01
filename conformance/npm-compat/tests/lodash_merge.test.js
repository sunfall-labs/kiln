// kiln eval smoke test (authored for kiln, not from porffor).
const merge = require('lodash.merge');
console.log(JSON.stringify(merge({a: {b: 1}, list: [1]}, {a: {c: 2}, list: [2, 3]})));
console.log(JSON.stringify(merge({}, {x: [1, 2]}, {x: [3]})));
console.log(JSON.stringify(merge({n: null}, {n: {deep: true}})));
