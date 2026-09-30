// kiln eval smoke test (authored for kiln, not from porffor).
const pako = require('pako');
const dec = new TextDecoder();
const input = 'hello world hello world hello world';
const deflated = pako.deflate(input);
const restored = dec.decode(pako.inflate(deflated));
console.log(restored === input, deflated.length < input.length);
const gz = pako.gzip(input);
console.log(dec.decode(pako.ungzip(gz)) === input);
console.log(pako.deflate('').length);
console.log([...pako.inflate(pako.deflate(new Uint8Array([1, 2, 3])))].join(','));
console.log(dec.decode(pako.inflateRaw(pako.deflateRaw('raw test'))));
