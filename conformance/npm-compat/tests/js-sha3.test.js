// kiln eval smoke test (authored for kiln, not from porffor).
const {sha3_256, keccak256, sha3_512} = require('js-sha3');
console.log(sha3_256('hello'));
console.log(keccak256(''));
console.log(sha3_512('abc').slice(0, 32));
const h = sha3_256.create();
h.update('hel');
h.update('lo');
console.log(h.hex());
console.log(h.hex() === sha3_256('hello') ? 'same' : 'diff');
