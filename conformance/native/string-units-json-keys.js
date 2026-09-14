// JSON and property-key unit coverage for the shared Latin1/UTF-16 representation.
// Every assertion observes UTF-16 code units, so a lone surrogate cannot be hidden by printing.

function units(text) {
  const result = [];
  for (let index = 0; index < text.length; index += 1) result.push(text.charCodeAt(index));
  return result;
}

function check(condition, message) {
  if (!condition) throw new Error(message);
}

function show(label, text) {
  console.log(label, JSON.stringify(units(text)), text.length);
}

const latin1Keys = ["é", "\x80", "ÿ", "\u0000"];
const latin1Values = ["é", "\x80", "ÿ", "\u0000"];
const keyed = {};
for (let index = 0; index < latin1Keys.length; index += 1) {
  keyed[latin1Keys[index]] = latin1Values[index];
}
console.log("latin1 keys", JSON.stringify(Object.keys(keyed).map(units)));
for (let index = 0; index < latin1Keys.length; index += 1) {
  show("latin1 value " + index, keyed[latin1Keys[index]]);
}

const parsedLatin1 = JSON.parse('"\\u00e9\\u0080\\u00ff\\u0000"');
show("parsed latin1", parsedLatin1);
const roundTripLatin1 = JSON.parse(JSON.stringify(keyed));
for (const key of latin1Keys) {
  check(JSON.stringify(units(roundTripLatin1[key])) === JSON.stringify(units(keyed[key])),
        "latin1 round trip " + JSON.stringify(units(key)));
}

const highLast = JSON.parse('"\\ud800"');
const highBeforeText = JSON.parse('"\\ud800x"');
const low = JSON.parse('"\\udc00"');
const pair = JSON.parse('"\\ud83d\\ude00"');
show("high last", highLast);
show("high before text", highBeforeText);
show("low", low);
show("pair", pair);
console.log("surrogate JSON", JSON.stringify(highLast), JSON.stringify(highBeforeText), JSON.stringify(low), JSON.stringify(pair));
check(JSON.stringify(highLast) === '"\\ud800"', "stringify trailing high surrogate");
check(JSON.stringify(low) === '"\\udc00"', "stringify lone low surrogate");
check(highLast.charCodeAt(0) === 0xd800, "trailing high surrogate");
check(highBeforeText.charCodeAt(0) === 0xd800 && highBeforeText.charCodeAt(1) === 0x78,
      "non-final high surrogate");
check(low.charCodeAt(0) === 0xdc00, "lone low surrogate");
check(pair.length === 2 && pair.charCodeAt(0) === 0xd83d && pair.charCodeAt(1) === 0xde00,
      "surrogate pair");

// A wide string sliced down to ASCII exercises a UTF-16-backed value whose spelling is narrow.
const wideAscii = String.fromCharCode(0x0100, 0x41, 0x0100).slice(1, 2);
show("wide slice ascii", wideAscii);
const wideKeyObject = { [wideAscii]: wideAscii };
check(wideKeyObject.A === "A", "wide slice key");
console.log("wide slice object", JSON.stringify(wideKeyObject));

// More than the fast shape slots forces dictionary storage before the __proto__ data key.
const members = [];
for (let index = 0; index < 40; index += 1) members.push('"k' + index + '":' + index);
members.push('"__proto__":{"polluted":1}');
const manyKeys = JSON.parse("{" + members.join(",") + "}");
check(Object.getPrototypeOf(manyKeys) === Object.prototype, "JSON __proto__ prototype");
check(Object.keys(manyKeys).length === 41, "JSON dictionary key count");
check(Object.prototype.hasOwnProperty.call(manyKeys, "__proto__"), "JSON __proto__ own key");
check(manyKeys.__proto__.polluted === 1 && ({}).polluted === undefined, "JSON __proto__ data");
show("json proto key", Object.keys(manyKeys).find((key) => key === "__proto__"));
console.log("json proto", JSON.stringify(manyKeys).includes('"__proto__":{"polluted":1}'));

// Keep the deep walk in this fixture so the same file can be rerun with allocation stress.
let deepSource = "0";
for (let index = 0; index < 220; index += 1) deepSource = '{"next":' + deepSource + '}';
const deep = JSON.parse(deepSource);
let cursor = deep;
for (let index = 0; index < 220; index += 1) cursor = cursor.next;
check(cursor === 0, "deep JSON walk");
console.log("deep json", JSON.stringify(deep).length, cursor);
