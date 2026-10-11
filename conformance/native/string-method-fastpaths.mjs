// String and join fast paths: results must match node, including when the built-ins are
// patched (also between a method's lookup and its call), receivers are String objects, and
// strings are non-ASCII.
const line = "a b  c d=1 amount=149.99 x";
const out = [];
function run(label, fn) {
  try { out.push(label + ": " + JSON.stringify(fn())); } catch (e) { out.push(label + " threw " + e.constructor.name); }
}
run("split", () => line.split(" "));
run("split limit", () => line.split(" ", 3));
run("split empty", () => "abc".split(""));
run("split none", () => "abc".split());
run("split sym", () => "abc".split({ [Symbol.split](s, l) { return [s, l, 5]; } }));
run("split nonascii", () => "é ü\u{1F600} x".split(" "));
run("split lone", () => "a\uD800 b".split(" ").map((s) => s.length));
run("join", () => line.split(" ").slice(2).join("&"));
run("join nonascii", () => ["é", "b", "\u{1F600}"].join("-"));
run("join nullish", () => ["a", null, undefined, "b"].join(","));
run("join mixed", () => ["a", 1, "b"].join(1));
run("join sparse", () => ["a", , "b"].join("+"));
run("join empty", () => [].join("x") + "|" + [""].join("x") + "|" + ["", ""].join("x"));
run("join nested", () => [["a", "b"], "c"].join(";"));
const idx = line.indexOf("amount=");
run("substring", () => line.substring(idx + 7, idx + 13));
run("substring odd", () => [line.substring(3, 1), line.substring(-2, 4), line.substring(NaN, 3), line.substring(2.7, 5.2), line.substring({ valueOf() { return 4; } }, 6)]);
run("slice odd", () => [line.slice(-4), line.slice(2.5), line.slice(NaN), line.slice({ valueOf() { return 3; } }), line.slice(1, -1)]);
run("array slice", () => line.split(" ").slice(2.5).length + "," + line.split(" ").slice(-2).length);
run("String object", () => { const s = new String("a b c"); return [s.split(" "), s.indexOf("b"), s.substring(1, 3), s.slice(2)]; });
// patched built-ins
const origSplit = String.prototype.split;
String.prototype.split = function () { return 7; };
run("patched split", () => line.split(" "));
run("patched split then slice", () => { const p = line.split(" "); return typeof p.slice; });
String.prototype.split = origSplit;
const origIndexOf = String.prototype.indexOf;
String.prototype.indexOf = function () { return 42; };
run("patched indexOf", () => line.indexOf("amount="));
String.prototype.indexOf = origIndexOf;
const origJoin = Array.prototype.join;
Array.prototype.join = function () { return "J"; };
run("patched join", () => ["a", "b"].join(","));
Array.prototype.join = origJoin;
// patched between the member lookup and the call (arguments run after the lookup)
const s2 = "hello world";
run("patched in args", () => {
  const saved = String.prototype.substring;
  const r = s2.substring((String.prototype.substring = function () { return "P"; }, 1), 4);
  String.prototype.substring = saved;
  return r;
});
run("patched in args slice", () => {
  const saved = String.prototype.slice;
  const r = s2.slice((String.prototype.slice = function () { return "Q"; }, 1), 4);
  String.prototype.slice = saved;
  return r;
});
run("own join", () => { const a = ["x", "y"]; a.join = () => "own"; return a.join(","); });
console.log(out.join("\n"));
