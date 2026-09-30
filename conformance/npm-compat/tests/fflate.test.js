// kiln eval smoke test (authored for kiln, not from porffor).
const { strToU8, strFromU8, compressSync, decompressSync, gzipSync, gunzipSync, zipSync, unzipSync } = require('fflate');

const text = 'hello kiln! '.repeat(100);
const u8 = strToU8(text);
const comp = compressSync(u8);
console.log('deflate:', u8.length, '->', comp.length, 'roundtrip:', strFromU8(decompressSync(comp)) === text);

const gz = gzipSync(u8);
console.log('gzip:', gz.length, 'roundtrip:', strFromU8(gunzipSync(gz)) === text);

const zipped = zipSync({ 'a.txt': strToU8('file a'), 'b.txt': strToU8('file b contents') });
const unzipped = unzipSync(zipped);
console.log('zip files:', JSON.stringify(Object.keys(unzipped).sort()));
console.log('zip content:', strFromU8(unzipped['b.txt']));

console.log('level 9 smaller:', compressSync(u8, { level: 9 }).length <= comp.length);
console.log('unicode roundtrip:', strFromU8(decompressSync(compressSync(strToU8('héllo 🎉')))) === 'héllo 🎉');
