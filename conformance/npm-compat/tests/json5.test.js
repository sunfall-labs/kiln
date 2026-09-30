// kiln eval smoke test (authored for kiln, not from porffor).
const JSON5 = require('json5');
console.log(JSON.stringify(JSON5.parse("{a: 1, b: 'two', c: [1, 2,], d: .5}")));
console.log(JSON5.stringify({ a: 1 }));
console.log(JSON.stringify(JSON5.parse('// comment\n{unquoted: true}')));
console.log(JSON.stringify(JSON5.parse('{hex: 0xdecaf, inf: Infinity}')));
console.log(JSON5.stringify({ trailing: 'comma' }, null, 2).split('\n').length);
