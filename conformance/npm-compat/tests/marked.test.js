// kiln eval smoke test (authored for kiln, not from porffor).
const { marked } = require('marked');

console.log('heading:', marked('# Hello *world*').trim());
console.log('link:', marked('[x](https://example.com)').trim());
console.log('code fence:', JSON.stringify(marked('```js\nconst a = 1;\n```')));
console.log('table:', marked('| a | b |\n|---|---|\n| 1 | 2 |').includes('<table>'));
console.log('inline lexer tokens:', marked.lexer('**bold**').length > 0);
const tokens = marked.lexer('# T\n\npara');
console.log('token types:', JSON.stringify(tokens.map((t) => t.type)));
console.log('parseInline:', marked.parseInline('`code` and *em*'));
console.log('use extension ok:', typeof marked.use === 'function');
