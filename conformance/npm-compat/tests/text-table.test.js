// kiln eval smoke test (authored for kiln, not from porffor).
const table = require('text-table');

console.log(table([
  ['name', 'age', 'city'],
  ['ann', '30', 'provo'],
  ['bob', '4', 'orem']
]));
console.log('---');
console.log(table([
  ['a', 'b'],
  ['longer', 'x']
], { align: ['l', 'r'] }));
console.log('---');
console.log(table([['one'], ['two'], ['three']], { hsep: ' | ' }));
