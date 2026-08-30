// A surrogate pair is two `\\u` escapes and one character. kiln decoded each half on its own, so
// every emoji that arrived escaped -- which is how any other engine spells one outside the BMP --
// came back as two replacement characters. Found by the native backend, which got it right.
const pair = JSON.parse('"\\ud83d\\ude00"');
console.log(pair.length, pair.charCodeAt(0), pair.charCodeAt(1), pair === "\u{1F600}");
console.log(JSON.parse('"a\\ud83d\\ude00b"').length, JSON.parse('"\\u0041\\u0042"'));
console.log(JSON.parse('["\\ud83d\\ude00", "\\u00e9"]'), JSON.parse('{"\\ud83d\\ude00":1}'));

// A high half with nothing after it, and a low half on its own, are unpaired: they have no scalar
// value, so they become the replacement character rather than failing the whole parse.
console.log(JSON.parse('"\\ud83d"').length, JSON.parse('"\\ude00"').length);
console.log(JSON.parse('"\\ud83dx"').length, JSON.parse('"\\ud83d\\ud83d"').length);

// The round trip through both directions.
const emoji = "a\u{1F600}b";
console.log(JSON.parse(JSON.stringify(emoji)) === emoji, JSON.stringify(emoji).length);
