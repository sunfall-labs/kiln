// kiln eval smoke test (authored for kiln, not from porffor).
const parse = require('spdx-expression-parse');

console.log(JSON.stringify(parse('MIT')));
console.log(JSON.stringify(parse('Apache-2.0 OR MIT')));
const and = parse('Apache-2.0 WITH Classpath-exception-2.0');
console.log(JSON.stringify([and.license, and.exception]));
console.log(JSON.stringify(parse('GPL-2.0-or-later+')));
