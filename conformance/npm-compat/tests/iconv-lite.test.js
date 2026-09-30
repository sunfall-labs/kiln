// kiln eval smoke test (authored for kiln, not from porffor).
const iconv = require('iconv-lite');
const buf = iconv.encode('Привет', 'win1251');
console.log(buf.toString('hex'));
console.log(iconv.decode(buf, 'win1251'));
console.log(iconv.encode('hello', 'utf8').toString('hex'));
console.log(iconv.decode(Buffer.from('e4b8ade69687', 'hex'), 'utf8'));
console.log(iconv.encodingExists('latin1'));
