// kiln eval smoke test (authored for kiln, not from porffor).
const { Base64 } = require('js-base64');
console.log(Base64.encode('hello world'));
console.log(Base64.decode('aGVsbG8gd29ybGQ='));
console.log(Base64.encode('✓ unicode ✓'));
console.log(Base64.decode(Base64.encode('roundtrip')));
console.log(Base64.atob('aGk='));
console.log(Base64.encodeURI('a+b/c'));
