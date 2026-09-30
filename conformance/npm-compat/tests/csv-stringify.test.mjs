// kiln eval smoke test (authored for kiln, not from porffor).
import { stringify } from 'csv-stringify/sync';
console.log(JSON.stringify(stringify([[1, 2], [3, 4]])));
console.log(JSON.stringify(stringify([{ a: 1, b: 2 }], { header: true })));
console.log(JSON.stringify(stringify([['x;y']], { delimiter: ';' })));
console.log(JSON.stringify(stringify([['a"b']])));
