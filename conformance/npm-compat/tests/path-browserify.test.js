// kiln eval smoke test (authored for kiln, not from porffor).
const path = require('path-browserify');
console.log(path.join('/a', 'b', 'c'));
console.log(path.extname('file.txt'));
console.log(path.basename('/a/b/c.js'));
console.log(path.dirname('/a/b/c.js'));
console.log(path.normalize('/a//b/../c'));
console.log(path.relative('/a/b', '/a/b/c/d'));
console.log(path.isAbsolute('/a/b'));
