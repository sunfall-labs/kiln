// kiln eval smoke test (authored for kiln, not from porffor).
const {parse} = require('meriyah');
console.log(parse('const x = 1 + 2;').body[0].type);
console.log(parse('function f(a) { return a * 2; }').body[0].id.name);
console.log(parse('class A extends B {}').body[0].superClass.name);
try { parse('const = 1;'); } catch (e) { console.log('parse-error:', e instanceof Error); }
console.log(parse('[1, 2].map(n => n + 1);').body[0].expression.callee.property.name);
console.log(parse('export default 42;', {module: true}).body[0].type);
