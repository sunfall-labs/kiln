// String's RegExp methods delegate to an argument's well-known protocol before coercing `this`.
const assert = (condition, label) => {
  if (!condition) throw new Error(`string protocol: ${label}`);
};

let coercions = 0;
const receiver = { [Symbol.toPrimitive]() { coercions++; return "subject"; } };
const exercise = (method, symbol, args, expected) => {
  const pattern = {
    [symbol](...forwarded) {
      assert(this === pattern, `${method} original pattern receiver`);
      assert(forwarded[0] === receiver, `${method} original this argument`);
      assert(coercions === 0, `${method} delegates before ToString(this)`);
      return expected;
    },
  };
  coercions = 0;
  assert(String.prototype[method].call(receiver, pattern, ...args) === expected, `${method} return`);
  assert(coercions === 0, `${method} does not coerce delegated receiver`);
};

exercise("match", Symbol.match, [], "matched");
exercise("matchAll", Symbol.matchAll, [], "all");
exercise("replace", Symbol.replace, ["replacement"], "replaced");
exercise("search", Symbol.search, [], 17);
exercise("split", Symbol.split, [3], "split");

let order = "";
const replaceAllPattern = {
  get [Symbol.match]() { order += "m"; return false; },
  [Symbol.replace](value, replacement) {
    order += "r";
    assert(value === receiver && replacement === "x", "replaceAll forwards original arguments");
    return "replaceAll";
  },
};
assert(String.prototype.replaceAll.call(receiver, replaceAllPattern, "x") === "replaceAll", "replaceAll result");
assert(order === "mr" && coercions === 0, "replaceAll checks IsRegExp before @@replace without coercing this");

for (const [name, length] of [["match", 1], ["matchAll", 1], ["replace", 2],
                              ["replaceAll", 2], ["search", 1], ["split", 2]]) {
  const descriptor = Object.getOwnPropertyDescriptor(String.prototype, name);
  assert(descriptor && descriptor.value.name === name && descriptor.value.length === length,
    `${name} keeps its builtin name and arity`);
  assert(descriptor.writable && !descriptor.enumerable && descriptor.configurable,
    `${name} keeps its builtin descriptor`);
}

let coerced = false;
const coercionProbe = { [Symbol.toPrimitive]() { coerced = true; return "x"; } };
let threw = false;
try { String.prototype.match.call(coercionProbe, { [Symbol.match]: 1 }); }
catch (error) { threw = error instanceof TypeError; }
assert(threw && !coerced, "non-callable protocol throws before receiver coercion");

let observed = false;
const throwingProtocol = Object.defineProperty({}, Symbol.search, {
  get() { observed = true; throw new Error("protocol getter"); },
});
threw = false;
try { String.prototype.search.call(receiver, throwingProtocol); }
catch (error) { threw = error.message === "protocol getter"; }
assert(threw && !coerced, "protocol getter exception propagates before ToString(this)");
observed = false;
threw = false;
try { String.prototype.match.call(null, Object.defineProperty({}, Symbol.match, {
  get() { observed = true; return () => 0; },
})); }
catch (error) { threw = error instanceof TypeError; }
assert(threw && !observed, "null receiver is rejected before protocol lookup");

assert(JSON.stringify("aba".match(/a/g)) === JSON.stringify(["a", "a"]), "regexp @@match raw path");
assert("aba".replace(/a/g, "x") === "xbx", "regexp @@replace raw path");
assert("aba".search(/b/) === 1, "regexp @@search raw path");
assert(JSON.stringify("aba".split(/b/)) === JSON.stringify(["a", "a"]), "regexp @@split raw path");
assert(Array.from("aba".matchAll(/a/g)).length === 2, "regexp @@matchAll raw path");

const oldMatch = RegExp.prototype[Symbol.match];
const oldSearch = RegExp.prototype[Symbol.search];
let createdOrder = "";
RegExp.prototype[Symbol.match] = function (value) {
  assert(this instanceof RegExp, "match fallback creates a RegExp receiver");
  createdOrder += "m";
  return value;
};
RegExp.prototype[Symbol.search] = function (value) {
  assert(this instanceof RegExp, "search fallback creates a RegExp receiver");
  createdOrder += "s";
  return value.length;
};
assert(String.prototype.match.call(receiver, "x") === "subject", "match RegExpCreate fallback");
assert(String.prototype.search.call(receiver, "x") === 7, "search RegExpCreate fallback");
assert(JSON.stringify(String.prototype.split.call("aba", "b")) === JSON.stringify(["a", "a"]),
  "split string fallback");
assert(createdOrder === "ms", "match/search fallbacks invoke current RegExp symbol methods");
RegExp.prototype[Symbol.match] = oldMatch;
RegExp.prototype[Symbol.search] = oldSearch;
console.log("String protocol delegation and raw RegExp fallback pass");
