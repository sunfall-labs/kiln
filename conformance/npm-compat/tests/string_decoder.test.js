// kiln eval smoke test (authored for kiln, not from porffor).
const {StringDecoder} = require('string_decoder');
const d = new StringDecoder('utf8');
console.log(JSON.stringify(d.write(Buffer.from([0xe2, 0x9c]))));
console.log(JSON.stringify(d.write(Buffer.from([0x93]))));
console.log(JSON.stringify(d.end()));
const d2 = new StringDecoder('utf16le');
console.log(JSON.stringify(d2.write(Buffer.from('hi', 'utf16le'))));
const d3 = new StringDecoder('utf8');
console.log(JSON.stringify(d3.write(Buffer.from('hello'))));
