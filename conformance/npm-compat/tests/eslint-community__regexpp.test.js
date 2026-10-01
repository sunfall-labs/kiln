// kiln eval smoke test (authored for kiln, not from porffor).
const {RegExpParser, RegExpValidator} = require('@eslint-community/regexpp');
const parser = new RegExpParser();
const ast = parser.parsePattern('a|b', 'u');
console.log(ast.type, ast.alternatives.length);
try {
  parser.parsePattern('(?<a>x)(?<a>y)', 'u');
} catch (e) {
  console.log('throws:' + e.message.slice(0, 45));
}
console.log(parser.parseFlags('gi').unicode);
console.log(new RegExpValidator().validatePattern('(?:x)+', 'u'));
