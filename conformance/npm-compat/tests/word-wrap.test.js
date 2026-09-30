// kiln eval smoke test (authored for kiln, not from porffor).
const wrap = require('word-wrap');
console.log(JSON.stringify(wrap('hello world foo bar baz', { width: 10 })));
console.log(JSON.stringify(wrap('one two three', { width: 7, indent: '  ' })));
console.log(JSON.stringify(wrap('supercalifragilisticexpialidocious ok', { width: 10 })));
console.log(JSON.stringify(wrap('a b c', { width: 80 })));
