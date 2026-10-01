// kiln npm functional test: acorn
const acorn = require('acorn');

// 1. Modern ECMAScript parsing: classes, optional chaining, nullish coalescing, destructuring
const code = 'class Calc { #val = 42; get() { return this.#val ?? 0; } } const { a = 1, ...rest } = obj?.prop ?? {};';
const ast = acorn.parse(code, { ecmaVersion: 2022, locations: true });
console.log('ast-body-count:', ast.body.length);
console.log('first-node-type:', ast.body[0].type);
console.log('class-id:', ast.body[0].id.name);
const classElements = ast.body[0].body.body.map(el => el.type);
console.log('class-elements:', JSON.stringify(classElements));

// 2. Tokenizer stream iteration
const tokens = [];
for (const token of acorn.tokenizer('let [x, y] = [10, 20];', { ecmaVersion: 2020 })) {
  tokens.push(token.type.label + (token.value !== undefined ? ':' + token.value : ''));
}
console.log('tokens:', JSON.stringify(tokens));

// 3. Comments and line locations
const comments = [];
acorn.parse('// line comment\n/* block */ const z = 1;', {
  ecmaVersion: 2020,
  onComment: (isBlock, text) => comments.push({ isBlock, text: text.trim() })
});
console.log('comments:', JSON.stringify(comments));

// 4. Intentional syntax error catching
let caughtError = null;
try {
  acorn.parse('const 123bad = true;', { ecmaVersion: 2020 });
} catch (err) {
  caughtError = { message: err.message.slice(0, 30), raisedAt: err.raisedAt };
}
console.log('syntax-error:', JSON.stringify(caughtError));
