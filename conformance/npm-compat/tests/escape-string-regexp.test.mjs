// kiln eval smoke test (authored for kiln, not from porffor).
import escapeStringRegexp from 'escape-string-regexp';
console.log(escapeStringRegexp('How much $ for a 🦄? (a+b)*[c]'));
console.log(escapeStringRegexp('a.b|c^d$e'));
console.log(new RegExp(escapeStringRegexp('(a+b)')).test('(a+b)'));
console.log(new RegExp(escapeStringRegexp('(a+b)')).test('aab'));
