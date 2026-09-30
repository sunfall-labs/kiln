// kiln eval smoke test (authored for kiln, not from porffor).
const normalizePath = require('normalize-path');
console.log(normalizePath('a\\b\\c'));
console.log(normalizePath('C:\\Users\\x'));
console.log(normalizePath('/a//b/./c'));
console.log(normalizePath('a/b/', true));
console.log(normalizePath('./x/../y'));
