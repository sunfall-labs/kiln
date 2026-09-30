// kiln eval smoke test (authored for kiln, not from porffor).
const Papa = require('papaparse');
console.log(JSON.stringify(Papa.parse('a,b\n1,2\n3,4', {header: true}).data));
console.log(JSON.stringify(Papa.parse('x;y\n1;2', {delimiter: ';', header: true}).data));
console.log(Papa.unparse([{a: 1, b: 2}, {a: 3, b: 4}]));
console.log(JSON.stringify(Papa.parse('a\n"q,q"\n2').data));
console.log(Papa.parse('bad\n1,2,3', {header: true}).errors.length >= 0);
