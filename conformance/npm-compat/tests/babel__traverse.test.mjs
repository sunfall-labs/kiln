// kiln eval smoke test (authored for kiln, not from porffor).
import parser from '@babel/parser';
import traverse from '@babel/traverse';
const ast = parser.parse('const x = 1; const y = x + 2;');
const ids = [];
traverse(ast, {Identifier(path) { ids.push(path.node.name); }});
console.log(ids.join(','));
let count = 0;
traverse(ast, {NumericLiteral() { count++; }});
console.log(count);
traverse(parser.parse('let a = 1; a = 2;'), {
  AssignmentExpression(path) { console.log(path.node.operator); },
});
