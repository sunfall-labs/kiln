const etag = require('etag');

console.log('--- String and Buffer ETags ---');
console.log('strong string:', etag('hello world'));
console.log('weak string:', etag('hello world', { weak: true }));
console.log('empty string:', etag(''));
console.log('utf8 string:', etag('🔥🚀'));
console.log('buffer:', etag(Buffer.from([0x01, 0x02, 0x03, 0x04])));
console.log('deterministic:', etag('hello world') === etag('hello world'));

console.log('--- File Stats-like Objects ---');
const fakeStat = {
  ctime: new Date('2026-01-01T00:00:00.000Z'),
  mtime: new Date('2026-01-01T00:00:00.000Z'),
  ino: 12345,
  size: 1048576
};
console.log('stat etag (default weak):', etag(fakeStat));
console.log('stat etag (explicit strong):', etag(fakeStat, { weak: false }));

console.log('--- Error Handling ---');
try { etag(123); } catch (e) { console.log('caught number'); }
try { etag(null); } catch (e) { console.log('caught null'); }
try { etag({}); } catch (e) { console.log('caught plain object'); }
