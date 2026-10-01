// kiln eval smoke test (authored for kiln, not from porffor).
const LZString = require('lz-string');

const text = 'the quick brown fox jumps over the lazy dog. '.repeat(10);
const b64 = LZString.compressToBase64(text);
console.log('b64 roundtrip:', LZString.decompressFromBase64(b64) === text, 'len:', b64.length, '<', text.length);
const utf16 = LZString.compressToUTF16(text);
console.log('utf16 roundtrip:', LZString.decompressFromUTF16(utf16) === text);
const uri = LZString.compressToEncodedURIComponent(text);
console.log('uri roundtrip:', LZString.decompressFromEncodedURIComponent(uri) === text);
const raw = LZString.compress(text);
console.log('raw roundtrip:', LZString.decompress(raw) === text);
console.log('empty:', JSON.stringify(LZString.decompressFromBase64(LZString.compressToBase64(''))));
console.log('unicode roundtrip:', LZString.decompressFromBase64(LZString.compressToBase64('héllo wörld 🎉')) === 'héllo wörld 🎉');
