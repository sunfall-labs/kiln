// kiln eval smoke test (authored for kiln, not from porffor).
const esprima = require('esprima');
const ast = esprima.parseScript('const x = 1 + 2;');
console.log(ast.body[0].type, ast.body[0].declarations[0].init.operator);
console.log(esprima.tokenize('a + b').map((t) => t.type).join(','));
console.log(esprima.parseModule('export default 1;').body[0].type);
console.log(esprima.parseScript('function f() { return 1; }').body[0].id.name);
