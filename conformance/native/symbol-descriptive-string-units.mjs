function check(actual, expected, label) {
  if (actual !== expected) throw new Error(label);
}
const keys = ["", "\0", "\u00ff", "\ud800", "\udc00", "x\ud800y", "x\udc00y", "\ud83d\ude00", "\ufffd", "\ud800\ud800"];
for (const key of keys) {
  for (const symbol of [Symbol(key), Symbol.for(key)]) {
    const expected = "Symbol(" + key + ")";
    check(String(symbol), expected, "String preserves description units");
    check(symbol.toString(), expected, "toString preserves description units");
    check(Symbol.prototype.toString.call(Object(symbol)), expected, "boxed toString preserves units");
    check(symbol.description, key, "description preserves units");
  }
  const symbol = Symbol.for(key);
  check(Symbol.keyFor(symbol), key, "registry preserves units");
  check(Symbol.for(/^Symbol\(([\s\S]*)\)$/.exec(String(symbol))[1]), symbol, "symbol string codec round trips");
}
check(String(Symbol()), "Symbol()", "absent description");
check(String(Symbol.iterator), "Symbol(Symbol.iterator)", "well-known description");
check(Symbol.for("\ud800") === Symbol.for("\ufffd"), false, "distinct registry keys");
console.log("symbol description UTF-16", keys.length);
