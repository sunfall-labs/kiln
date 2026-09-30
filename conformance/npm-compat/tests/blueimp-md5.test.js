// kiln eval smoke test (authored for kiln, not from porffor).
const md5 = require('blueimp-md5');
console.log(md5('hello'));
console.log(md5(''));
console.log(md5('The quick brown fox jumps over the lazy dog'));
console.log(md5('hello') === '5d41402abc4b2a76b9719d911017c592');
