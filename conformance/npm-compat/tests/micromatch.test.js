// kiln eval smoke test (authored for kiln, not from porffor).
const micromatch = require('micromatch');
console.log(JSON.stringify(micromatch(['a.js', 'b.txt', 'c.js'], '*.js')));
console.log(micromatch.isMatch('a/b.js', 'a/*.js'));
console.log(micromatch.isMatch('a/b/c.js', 'a/**/*.js'));
console.log(JSON.stringify(micromatch(['a.js', 'b.js'], ['*.js', '!b.js'])));
console.log(micromatch.capture('test/*.js', 'test/foo.js').join(','));
console.log(JSON.stringify(micromatch.braces('a/{x,y}.js')));
