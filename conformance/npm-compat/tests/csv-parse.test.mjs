// kiln eval smoke test (authored for kiln, not from porffor).
import { parse } from 'csv-parse/sync';
console.log(JSON.stringify(parse('a,b\n1,2\n3,4')));
console.log(JSON.stringify(parse('a,b\n1,2', { columns: true })));
console.log(JSON.stringify(parse('x;y\n1;2', { delimiter: ';' })));
console.log(JSON.stringify(parse('"q, u"\n1', { columns: false })[0]));
