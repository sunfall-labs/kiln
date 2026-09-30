// kiln eval smoke test (authored for kiln, not from porffor).
import { parse } from 'espree';
const ast = parse('const x = 1 + 2;', { ecmaVersion: 'latest' });
console.log(ast.type, ast.body[0].type, ast.body[0].declarations[0].id.name);
const fn = parse('function f(a) { return a * 2; }', { ecmaVersion: 'latest' });
console.log(fn.body[0].type, fn.body[0].id.name, fn.body[0].params.length);
console.log(parse('1 + 2 * 3', { ecmaVersion: 'latest' }).body[0].expression.operator);
