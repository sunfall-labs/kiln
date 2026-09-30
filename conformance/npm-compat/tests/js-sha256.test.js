// kiln eval smoke test (authored for kiln, not from porffor).
const sha256 = require('js-sha256');
console.log(sha256('hello'));
console.log(sha256(''));
console.log(sha256.hex('abc'));
console.log(sha256.array('hi').length);
console.log(sha256('hello') === '2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824');
