// kiln eval smoke test (authored for kiln, not from porffor).
const recast = require('recast');
const { namedTypes: n, builders: b } = recast.types;

const ast = recast.parse('const x = 1;\nfoo(x);');
console.log(ast.program.body[0].type);
console.log(ast.program.body[1].expression.callee.name);

const id = b.identifier('foo');
console.log(n.Identifier.check(id));
console.log(n.FunctionDeclaration.check(id));

const decl = b.variableDeclaration('const', [b.variableDeclarator(id, b.literal(42))]);
console.log(recast.print(decl).code);

const parsed = recast.parse('1 + 2');
console.log(recast.prettyPrint(parsed).code);
