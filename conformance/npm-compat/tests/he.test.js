const he = require('he');

console.log('--- Encoding with options ---');
console.log('encode basic:', he.encode('Tom & Jerry <3 "quotes"'));
console.log('encode named:', he.encode('© 2026 — «hi»', { useNamedReferences: true }));
console.log('encode decimal:', he.encode('© 2026', { decimal: true }));
console.log('encode everything:', he.encode('abc', { encodeEverything: true }));
console.log('encode allowUnsafeSymbols:', he.encode('<b>"test"</b>', { allowUnsafeSymbols: true }));

console.log('--- Decoding numeric and named ---');
console.log('decode named:', he.decode('Tom &amp; Jerry &lt;3 &quot;quotes&quot;'));
console.log('decode hex and dec:', he.decode('&#65;&#x42;&#x43;'));
console.log('decode invalid numeric entity:', he.decode('&#9999999;'));

console.log('--- Astral symbols / Emojis ---');
const emojiText = 'Kiln 🚀 is fast 🔥';
const encodedEmoji = he.encode(emojiText, { encodeEverything: true });
console.log('encoded emoji length > original:', encodedEmoji.length > emojiText.length);
console.log('decoded emoji matches:', he.decode(encodedEmoji) === emojiText);

console.log('--- escape & unescape ---');
console.log('escape:', he.escape('<div class="box">&copy;</div>'));
console.log('unescape:', he.unescape('&lt;div class=&quot;box&quot;&gt;&amp;copy;&lt;/div&gt;'));

console.log('--- Strict vs Lenient error handling ---');
let strictThrew = false;
try {
  he.decode('&amp', { strict: true });
} catch (e) {
  strictThrew = true;
}
console.log('strict throws on missing semicolon:', strictThrew);
console.log('lenient handles missing semicolon:', he.decode('&amp', { strict: false }));
