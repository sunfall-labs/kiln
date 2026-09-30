// kiln eval smoke test (authored for kiln, not from porffor).
const _ = require('underscore');

console.log('map:', JSON.stringify(_.map([1, 2, 3], (x) => x * 2)));
console.log('reduce:', _.reduce([1, 2, 3, 4], (m, x) => m + x, 0));
console.log('filter:', JSON.stringify(_.filter([1, 2, 3, 4], (x) => x % 2 === 0)));
console.log('uniq:', JSON.stringify(_.uniq([1, 2, 2, 3, 1])));
console.log('groupBy:', JSON.stringify(_.groupBy([1.3, 2.1, 2.4], (n) => Math.floor(n))));
console.log('sortBy:', JSON.stringify(_.sortBy([{ n: 2 }, { n: 1 }], 'n')));
console.log('template:', _.template('hello <%= name %>!')({ name: 'kiln' }));
console.log('range:', JSON.stringify(_.range(5)));
console.log('VERSION:', _.VERSION);
