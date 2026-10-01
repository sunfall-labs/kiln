// kiln eval smoke test (authored for kiln, not from porffor).
const escodegen = require('escodegen');

const ast = {
  type: 'Program',
  body: [{
    type: 'VariableDeclaration',
    kind: 'const',
    declarations: [{
      type: 'VariableDeclarator',
      id: { type: 'Identifier', name: 'answer' },
      init: {
        type: 'BinaryExpression', operator: '*',
        left: { type: 'Literal', value: 6 },
        right: { type: 'Literal', value: 7 }
      }
    }]
  }]
};
console.log('generate:', escodegen.generate(ast));
console.log('compact:', escodegen.generate(ast, { format: { compact: true } }));

const fn = {
  type: 'FunctionDeclaration',
  id: { type: 'Identifier', name: 'add' },
  params: [{ type: 'Identifier', name: 'a' }, { type: 'Identifier', name: 'b' }],
  body: {
    type: 'BlockStatement',
    body: [{
      type: 'ReturnStatement',
      argument: {
        type: 'BinaryExpression', operator: '+',
        left: { type: 'Identifier', name: 'a' },
        right: { type: 'Identifier', name: 'b' }
      }
    }]
  }
};
console.log('function:', escodegen.generate(fn).replace(/\n/g, ' '));
console.log('attachComments is fn:', typeof escodegen.attachComments === 'function');
