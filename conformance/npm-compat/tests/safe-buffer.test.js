// kiln eval smoke test (authored for kiln, not from porffor).
const {Buffer} = require('safe-buffer');
console.log(Buffer.from('hello').toString('hex'));
console.log(Buffer.alloc(4, 'ab').toString());
console.log(Buffer.concat([Buffer.from('a'), Buffer.from('b')]).toString());
console.log(Buffer.isBuffer(Buffer.from('x')));
console.log(Buffer.from('✓').length);
