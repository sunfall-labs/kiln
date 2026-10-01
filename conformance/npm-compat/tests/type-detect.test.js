// kiln eval smoke test (authored for kiln, not from porffor).
const typeDetect = require('type-detect');

console.log(typeDetect(42));
console.log(typeDetect('str'));
console.log(typeDetect([1, 2]));
console.log(typeDetect({ a: 1 }));
console.log(typeDetect(null));
console.log(typeDetect(new Map()));
console.log(typeDetect(Promise.resolve()));
console.log(typeDetect(/re/g));
