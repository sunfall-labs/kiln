// kiln eval smoke test (authored for kiln, not from porffor).
const pm = require('picomatch');
console.log(pm.isMatch('src/index.js', '**/*.js'));
console.log(pm.isMatch('src/index.ts', '**/*.js'));
const isJs = pm('*.js');
console.log(isJs('a.js'), isJs('a.ts'));
console.log(pm.isMatch('foo/bar', 'foo/*'));
console.log(pm.isMatch('a/b/c', 'a/{b,c}/**'));
console.log(pm.isMatch('a/d/c', 'a/{b,c}/**'));
