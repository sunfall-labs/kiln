// kiln eval smoke test (authored for kiln, not from porffor).
const clsx = require('clsx');
console.log(JSON.stringify(clsx('a', 'b')));
console.log(JSON.stringify(clsx('a', false && 'b', 'c')));
console.log(JSON.stringify(clsx({ a: true, b: false, c: 1 })));
console.log(JSON.stringify(clsx(['a', ['b', 'c']])));
console.log(JSON.stringify(clsx('')));
console.log(JSON.stringify(clsx('a', 0, null, undefined, 'd')));
