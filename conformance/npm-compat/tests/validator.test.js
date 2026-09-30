// kiln eval smoke test (authored for kiln, not from porffor).
const validator = require('validator');

console.log('isEmail:', validator.isEmail('a@b.com'), validator.isEmail('not-an-email'));
console.log('isURL:', validator.isURL('https://example.com/x?q=1'), validator.isURL('notaurl'));
console.log('isIP:', validator.isIP('127.0.0.1'), validator.isIP('999.1.1.1'));
console.log('isUUID:', validator.isUUID('550e8400-e29b-41d4-a716-446655440000'));
console.log('isInt/isFloat:', validator.isInt('42'), validator.isFloat('4.2'));
console.log('isAlpha/isAlphanumeric:', validator.isAlpha('abc'), validator.isAlphanumeric('abc123'));
console.log('normalizeEmail:', validator.normalizeEmail('Test@Gmail.COM'));
console.log('escape:', validator.escape('<script>"x"</script>'));
console.log('isLength:', validator.isLength('hello', { min: 2, max: 10 }));
console.log('toInt/toFloat:', validator.toInt('42'), validator.toFloat('4.2'));
