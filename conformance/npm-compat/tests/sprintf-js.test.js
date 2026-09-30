// kiln eval smoke test (authored for kiln, not from porffor).
const { sprintf } = require('sprintf-js');

console.log(sprintf('%s is %d years old', 'Ann', 25));
console.log(sprintf('%05.2f', 3.14159));
console.log(sprintf('%x', 255));
console.log(sprintf('%(name)s: %(val)d', { name: 'key', val: 42 }));
console.log(sprintf('%2$s %1$s', 'first', 'second'));
