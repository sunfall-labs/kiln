// kiln eval smoke test (authored for kiln, not from porffor).
const {decode, encode} = require('@jridgewell/sourcemap-codec');
console.log(JSON.stringify(decode('AAAA')));
console.log(encode([[[0, 0, 0, 0]]]));
console.log(JSON.stringify(decode('CAAC,GAAG')));
console.log(JSON.stringify(decode('AAAAA')));
