// kiln eval smoke test (authored for kiln, not from porffor).
const isGlob = require('is-glob');
console.log(isGlob('*.js'), isGlob('foo.js'), isGlob('a/{b,c}.js'), isGlob('docs/**/*.md'), isGlob('plain.txt'));
console.log(isGlob('!negated.js'), isGlob('a.@(js|ts)'));
