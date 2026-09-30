// kiln eval smoke test (authored for kiln, not from porffor).
const globParent = require('glob-parent');
console.log(globParent('src/components/*.js'));
console.log(globParent('*.js'));
console.log(globParent('docs/guide/readme.md'));
console.log(globParent('a/{b,c}/file.js'));
console.log(globParent('src/**/deep/*.test.js'));
