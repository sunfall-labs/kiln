// kiln eval smoke test (authored for kiln, not from porffor).
const braces = require('braces');
console.log(JSON.stringify(braces.expand('a/{b,c}/d')));
console.log(JSON.stringify(braces.expand('x{1..3}')));
console.log(JSON.stringify(braces('a/{b,c}/d')));
console.log(JSON.stringify(braces.compile('a/{b,c}/d')));
console.log(JSON.stringify(braces.expand('file.{js,ts}')));
console.log(braces.parse('a/{b,c}').type);
