// kiln eval smoke test (authored for kiln, not from porffor).
const estraverse = require('estraverse');
const ast = {
  type: 'Program',
  body: [{type: 'ExpressionStatement', expression: {type: 'Identifier', name: 'x'}}],
};
const types = [];
estraverse.traverse(ast, {enter: (node) => types.push(node.type)});
console.log(types.join(','));
console.log(estraverse.Syntax.Identifier);
const replaced = estraverse.replace({type: 'Program', body: []}, {enter: (n) => n});
console.log(replaced.type);
console.log(estraverse.VisitorKeys.Program.join(','));
