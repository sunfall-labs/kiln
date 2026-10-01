// kiln npm functional test: pako
const pako = require('pako');

// 1. Chunked streaming Deflate / Inflate
const text = 'Kiln native ahead-of-time compiler with incremental caching. '.repeat(10);
const deflator = new pako.Deflate({ level: 6, chunkSize: 32 });
const chunks = [];
deflator.onData = (chunk) => chunks.push(chunk);
const inputBytes = new TextEncoder().encode(text);
for (let i = 0; i < inputBytes.length; i += 50) {
  const isLast = i + 50 >= inputBytes.length;
  deflator.push(inputBytes.subarray(i, i + 50), isLast);
}
const compressed = new Uint8Array(chunks.reduce((acc, c) => acc + c.length, 0));
let offset = 0;
for (const c of chunks) {
  compressed.set(c, offset);
  offset += c.length;
}
console.log('compression-ratio-good:', compressed.length < inputBytes.length);

const inflator = new pako.Inflate({ chunkSize: 32 });
const restoredChunks = [];
inflator.onData = (chunk) => restoredChunks.push(chunk);
for (let i = 0; i < compressed.length; i += 40) {
  const isLast = i + 40 >= compressed.length;
  inflator.push(compressed.subarray(i, i + 40), isLast);
}
const decompressed = new Uint8Array(restoredChunks.reduce((acc, c) => acc + c.length, 0));
offset = 0;
for (const c of restoredChunks) {
  decompressed.set(c, offset);
  offset += c.length;
}
console.log('streaming-roundtrip-match:', new TextDecoder().decode(decompressed) === text);

// 2. GZIP format with metadata header
const gz = pako.gzip(text, { header: { name: 'kiln.txt', os: 3 } });
const ungz = pako.ungzip(gz);
console.log('gzip-match:', new TextDecoder().decode(ungz) === text);

// 3. Raw Deflate without zlib wrapper
const rawDeflated = pako.deflateRaw('raw payload test');
const rawInflated = new TextDecoder().decode(pako.inflateRaw(rawDeflated));
console.log('raw-match:', rawInflated === 'raw payload test');

// 4. Corrupted data error handling
let caughtZlibError = false;
try {
  pako.inflate(new Uint8Array([0x78, 0x9c, 0xff, 0xff, 0xff, 0x00]));
} catch (e) {
  caughtZlibError = true;
}
console.log('corrupted-data-throws:', caughtZlibError);
