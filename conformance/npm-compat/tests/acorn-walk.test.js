// kiln eval smoke test (authored for kiln, not from porffor).
const acorn = require('acorn');
const walk = require('acorn-walk');

const ast = acorn.parse('const x = 1 + 2; function f(a) { return a * x; }', { ecmaVersion: 2022 });
const kinds = [];
walk.simple(ast, {
  VariableDeclaration(n) { kinds.push('var:' + n.kind); },
  FunctionDeclaration(n) { kinds.push('fn:' + n.id.name); },
  BinaryExpression(n) { kinds.push('bin:' + n.operator); }
});
console.log('simple:', JSON.stringify(kinds.sort()));

let literalCount = 0;
walk.full(ast, (n) => { if (n.type === 'Literal') literalCount++; });
console.log('literals:', literalCount);

const found = walk.findNodeAt(ast, 0, undefined, 'Program');
console.log('findNodeAt Program:', found && found.node.type);

const ancestors = [];
walk.ancestor(ast, {
  Identifier(n, st, anc) { if (n.name === 'x') ancestors.push(anc.length); }
});
console.log('ancestor depths for x:', JSON.stringify(ancestors));
