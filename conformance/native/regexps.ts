// Regular expressions. The engine is `regress`, linked as a provider archive rather than written
// twice (ADR 0025) -- so the interesting question is not whether the two engines agree about a
// backreference, which they cannot fail to, but whether everything *around* the engine agrees:
// the cursor, the match result, the replacement templates, and what a pattern prints as.

const plain = /a(b)?c/;
console.log(plain.source, plain.flags, plain.global, plain.sticky, plain.ignoreCase);
console.log(String(plain), `${plain}`, plain.toString(), typeof plain, plain instanceof RegExp);
console.log(/x/gimsuy.flags, /x/gi.global, /x/gi.ignoreCase, /x/y.sticky, /x/m.multiline, /x/s.dotAll);

// `exec` reports where it matched and what it matched with, as properties *of the array* -- which
// is why an array needs somewhere to keep something that is not an element.
const found = plain.exec("zzabc");
console.log(found, found![0], found![1], found!.index, found!.input, found!.groups, found!.length);
console.log(plain.exec("zzz"), plain.test("xabc"), plain.test("xxx"), plain.lastIndex);
const optional = /(a)?(b)/.exec("b");
console.log(optional![0], optional![1], optional![2], optional!.length);

// Only a `g` or `y` pattern has a cursor, and a miss resets it -- which is what makes a
// `while (re.exec(s))` loop terminate rather than repeat its last match forever.
const global = /a/g;
console.log(global.test("aa"), global.lastIndex, global.test("aa"), global.lastIndex);
console.log(global.test("aa"), global.lastIndex, global.exec("aa")?.index, global.lastIndex);
global.lastIndex = 0;
let rounds = 0;
while (global.exec("aaa") !== null) { rounds += 1; }
console.log("rounds", rounds, global.lastIndex);

// A sticky pattern must match exactly at the cursor, not merely at or after it.
const sticky = /b/y;
sticky.lastIndex = 1;
console.log(sticky.test("ab"), sticky.lastIndex);
sticky.lastIndex = 0;
console.log(sticky.test("ab"), sticky.lastIndex);

// The engine is the same one the interpreter runs, which is the whole reason for the provider.
console.log(/(\w)\1/.test("aa"), /(\w)\1/.test("ab"), /(?<=a)b/.test("ab"), /a(?!b)/.test("ac"));
console.log(/^\d{2,3}$/.test("12"), /^\d{2,3}$/.test("1"), /[^a-z]/.test("A"), /\bword\b/.test("a word here"));

// `new RegExp` takes text, so a pattern the frontend never saw can still be rejected -- and the
// rejection has to be catchable, because the text can come from the program's own input.
console.log(new RegExp("a+", "g").source, new RegExp(/x/i).flags, new RegExp(/x/i, "g").flags);
console.log(new RegExp("").source === "(?:)" || new RegExp("").source === "", RegExp("b").test("abc"));
try { new RegExp("("); console.log("no throw"); } catch (error: any) { console.log("bad", error.constructor.name); }

// is-regex's robust brand check uses both the own data descriptor and RegExpExec's ToString.
// The marker conversion throws; a non-RegExp must not accidentally pass this test.
const regexMarker = {};
const badStringifier = {
  toString() { throw regexMarker; },
  valueOf() { throw regexMarker; },
  [Symbol.toPrimitive]() { throw regexMarker; },
};
const lastIndexDescriptor = Object.getOwnPropertyDescriptor(/x/, "lastIndex");
let regexExecMarker = false;
try { /x/.exec(badStringifier as any); } catch (error) { regexExecMarker = error === regexMarker; }
console.log("regex-brand-primitives", lastIndexDescriptor?.value, lastIndexDescriptor?.writable,
  lastIndexDescriptor?.enumerable, lastIndexDescriptor?.configurable, regexExecMarker);

// The string methods that take a pattern. `replace` with a plain string was already here; these
// are the ones that need the engine, and `replace` with a *function* is the one that needs the
// interpreter -- which is why they are not where the other string methods are.
console.log("a1b2".replace(/\d/, "#"), "a1b2".replace(/\d/g, "#"), "a1b2".replaceAll(/\d/g, "#"));
console.log("ab".replace(/(a)(b)/, "$2$1"), "ab".replace(/a/, "$&$&"), "ab".replace(/b/, "[$`]"));
console.log("ab".replace(/a/, "[$']"), "x".replace(/x/, "$$"), "x".replace(/x/, "a$1b"));
console.log("a1b2".replace(/\d/g, (match) => `[${match}]`));
console.log("a1b2".replace(/(\d)/g, (match, group, index) => `${match}${group}${index}`));
console.log("abc".replace("b", (match: string) => match.toUpperCase()));
console.log("a,b;c".split(/[,;]/).join("|"), "a1b".split(/(\d)/).join("|"), "abc".split(/x/).length);
console.log("abc".search(/b/), "abc".search(/z/), "abc".search("c"));
console.log("a1b2".match(/\d/), "a1b2".match(/\d/g), "abc".match(/z/), "abc".match("b")![0]);
console.log([..."a1b2".matchAll(/(\d)/g)].map((m) => `${m[0]}@${m.index}`).join(","));
// An empty match advances by one, or a pattern that can match nothing would never finish.
console.log("abc".replace(/x*/g, "-"), "abc".split(/x*/).join("|"), [..."ab".matchAll(/x*/g)].length);
