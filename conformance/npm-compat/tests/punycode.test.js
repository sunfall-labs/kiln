// kiln eval smoke test (authored for kiln, not from porffor).
const punycode = require('punycode/');
console.log(punycode.toASCII('münchen.de'));
console.log(punycode.toUnicode('xn--mnchen-3ya.de'));
console.log(punycode.encode('ü'));
console.log(punycode.decode('tda'));
console.log(punycode.toASCII('bücher.de'));
