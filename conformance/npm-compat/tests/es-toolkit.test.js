// kiln eval smoke test (authored for kiln, not from porffor).
const { chunk, uniq, flattenDeep, groupBy, take } = require('es-toolkit');
console.log(JSON.stringify(chunk([1, 2, 3, 4, 5], 2)));
console.log(JSON.stringify(uniq([1, 2, 2, 3, 1])));
console.log(JSON.stringify(flattenDeep([1, [2, [3]]])));
console.log(JSON.stringify(groupBy(['one', 'two', 'three'], s => s.length)));
console.log(JSON.stringify(take([1, 2, 3, 4], 2)));
