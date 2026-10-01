// kiln eval smoke test (authored for kiln, not from porffor).
const diff = require('fast-diff');
console.log(JSON.stringify(diff('kitten', 'sitting')));
console.log(JSON.stringify(diff('abc', 'abc')));
console.log(JSON.stringify(diff('', 'x')));
console.log(JSON.stringify(diff('same', 'same')));
