// kiln eval smoke test (authored for kiln, not from porffor).
const { generate } = require('astring');

const ast = {
  type: 'Program',
  body: [{
    type: 'VariableDeclaration',
    kind: 'let',
    declarations: [{
      type: 'VariableDeclarator',
      id: { type: 'Identifier', name: 'x' },
      init: { type: 'Literal', value: 41 }
    }]
  }]
};
console.log('generate:', generate(ast).trim());

const expr = {
  type: 'ExpressionStatement',
  expression: {
    type: 'CallExpression',
    callee: { type: 'Identifier', name: 'f' },
    arguments: [{ type: 'Literal', value: 'hi' }, { type: 'Literal', value: 2 }]
  }
};
console.log('call:', generate(expr).trim());

const arrow = {
  type: 'VariableDeclaration',
  kind: 'const',
  declarations: [{
    type: 'VariableDeclarator',
    id: { type: 'Identifier', name: 'dbl' },
    init: {
      type: 'ArrowFunctionExpression',
      params: [{ type: 'Identifier', name: 'n' }],
      body: {
        type: 'BinaryExpression', operator: '*',
        left: { type: 'Identifier', name: 'n' },
        right: { type: 'Literal', value: 2 }
      }
    }
  }]
};
console.log('arrow:', generate({ type: 'Program', body: [arrow] }).trim());
console.log('custom indent:', JSON.stringify(generate(ast, { indent: '\t' }).split('\n')[0]));
