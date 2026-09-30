// kiln eval smoke test (authored for kiln, not from porffor).
const base64 = require('base64-js');
console.log(base64.fromByteArray(new Uint8Array([104, 105, 33])));
console.log(Array.from(base64.toByteArray('aGkh')).join(','));
console.log(base64.byteLength('aGk='));
console.log(base64.fromByteArray(new Uint8Array([255, 254])));
