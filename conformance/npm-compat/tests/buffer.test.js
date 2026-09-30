// kiln eval smoke test (authored for kiln, not from porffor).
const {Buffer} = require('buffer');
console.log(Buffer.from('Hello, Porffor! ✓', 'utf8').toString('hex'));
console.log(Buffer.from('48656c6c6f', 'hex').toString());
console.log(Buffer.alloc(3, 65).toString());
console.log(Buffer.byteLength('✓'));
console.log(Buffer.from('aGVsbG8=', 'base64').toString());
