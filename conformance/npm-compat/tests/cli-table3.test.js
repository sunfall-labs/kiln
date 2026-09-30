// kiln eval smoke test (authored for kiln, not from porffor).
const Table = require('cli-table3');
const t = new Table({head: ['a', 'b']});
t.push(['1', '2'], ['3', '4']);
const lines = t.toString().split('\n');
console.log(lines.length);
console.log(JSON.stringify(lines[1]));
const t2 = new Table({head: ['x'], colWidths: [10]});
t2.push(['yz']);
console.log(t2.toString().split('\n').length);
console.log(JSON.stringify(t2.toString().split('\n')[3]));
