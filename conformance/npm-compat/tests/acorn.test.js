// kiln eval smoke test (authored for kiln, not from porffor).
const acorn = require('acorn');
const ast = acorn.parse('const x = 1 + 2;', {ecmaVersion: 2020});
console.log(ast.body[0].type, ast.body[0].declarations[0].id.name);
console.log(acorn.parse('1 + 2 * 3', {ecmaVersion: 5}).body[0].expression.operator);
console.log(acorn.parseExpressionAt('a + b', 0, {ecmaVersion: 2020}).type);
console.log(JSON.stringify(acorn.getLineInfo('a\nb', 2)));
console.log(acorn.version);
