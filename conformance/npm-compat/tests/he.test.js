// kiln eval smoke test (authored for kiln, not from porffor).
const he = require('he');

console.log('encode:', he.encode('Tom & Jerry <3 "quotes"'));
console.log('encode named:', he.encode('© 2026 — «hi»', { useNamedReferences: true }));
console.log('decode:', he.decode('Tom &amp; Jerry &lt;3 &quot;quotes&quot;'));
console.log('decode numeric:', he.decode('&#65;&#x42;'));
console.log('escape:', he.escape('<div class="a">'));
console.log('roundtrip:', he.decode(he.encode('a<b>&c"d\'e')) === 'a<b>&c"d\'e');
let strictThrew = false;
try { he.decode('&amp', { strict: true }); } catch (e) { strictThrew = true; }
console.log('strict throws:', strictThrew, '| lenient:', he.decode('&amp', { strict: false }));
