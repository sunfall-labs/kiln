// kiln eval smoke test (authored for kiln, not from porffor).
const esquery = require('esquery');
const esprima = require('esprima');
const ast = esprima.parseScript('const x = 1; const y = x + 2;');
console.log(esquery(ast, 'Identifier').map((n) => n.name).join(','));
console.log(esquery(ast, 'VariableDeclaration').length);
console.log(esquery.matches(ast.body[0], esquery.parse('VariableDeclaration')));
console.log(esquery.matches(ast.body[0].declarations[0].id, esquery.parse('Identifier')));
console.log(esquery.matches(ast.body[0], esquery.parse('Identifier')));
console.log(esquery(ast, 'Identifier[name=x]').length);
console.log(esquery(ast, 'Identifier[name="x"]').length);
