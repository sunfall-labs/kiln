// kiln eval smoke test (authored for kiln, not from porffor).
const bytes = require('bytes');
console.log(bytes(1024));
console.log(bytes(1536));
console.log(bytes('1KB'));
console.log(bytes('1.5MB'));
console.log(bytes.format(123456789));
console.log(bytes.parse('512B'));
