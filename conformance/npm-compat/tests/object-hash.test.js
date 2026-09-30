// kiln eval smoke test (authored for kiln, not from porffor).
const hash = require('object-hash');
console.log(hash({ a: 1, b: 2 }));
console.log(hash({ b: 2, a: 1 }) === hash({ a: 1, b: 2 }));
console.log(hash('hello'));
console.log(hash({ a: 1 }, { algorithm: 'md5' }));
console.log(hash([1, 2, 3]));
console.log(hash(null));
