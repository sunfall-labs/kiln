// Parallel-builtins issue 30, item 1. A built-in prototype method the program replaced is the one
// every call reaches, natively as in the interpreter and node: after `String.prototype.trim = f`,
// `" z ".trim()` calls `f`, and `" z ".trim === String.prototype.trim`. Every call site below runs
// warm first -- its method cache and its fast path filled with the table's method -- then each
// member is replaced, then put back. `cat` joins without `Array.prototype.join`, and `note`
// records without `push`, since both are among the members replaced.
function cat(a, sep) { let out = ""; for (let i = 0; i < a.length; i++) out += (i ? sep : "") + String(a[i]); return out; }
const log = []; const note = (line) => { log[log.length] = line; };
function trims(s) { return s.trim(); }
function joins(a) { return a.join("-"); }
function fixed(n) { return n.toFixed(2); }
function slices(a) { return a.slice(1); }
function splits(s) { return s.split(","); }
function indexes(s) { return s.indexOf("b"); }
function pushes(a, x) { return a.push(x); }
function gets(m, k) { return m.get(k); }
function tests(re, s) { return re.test(s); }
function maps(a) { return a.map((x) => x * 2).join(","); }
function sorts(a) { return a.sort().join(","); }
function starts(s) { return s.startsWith("a"); }
function upper(s) { return s.toUpperCase(); }
function subs(s) { return s.substring(1, 2); }
function has(o) { return o.hasOwnProperty("x"); }
function round() {
  const m = new Map([["k", 1]]);
  return cat([trims(" z "), joins([1, 2]), fixed(1.5), slices([1, 2, 3]).length, splits("a,b").length,
          indexes("abc"), pushes([1], 2), gets(m, "k"), tests(/a/, "a"), maps([1, 2]), sorts([2, 1]),
          starts("abc"), upper("ab"), subs("abc"), has({ x: 1 }), has([1])], "|");
}
for (let i = 0; i < 50; i++) round();
note("pristine " + round());

const saved = {};
function patch(proto, name, fn) { saved[name] = proto[name]; proto[name] = fn; }
patch(String.prototype, "trim", function () { return "T(" + this + ")"; });
patch(Array.prototype, "join", function (s) { return "J" + this.length + s; });
patch(Number.prototype, "toFixed", function () { return "F" + this; });
patch(Array.prototype, "slice", function () { return "S"; });
patch(String.prototype, "split", function () { return "SP"; });
patch(String.prototype, "indexOf", function () { return -7; });
patch(Array.prototype, "push", function () { return "P"; });
patch(Map.prototype, "get", function (k) { return "G" + k; });
patch(RegExp.prototype, "test", function () { return "RT"; });
patch(Array.prototype, "map", function () { return ["M"]; });
patch(Array.prototype, "sort", function () { return ["SO"]; });
patch(String.prototype, "startsWith", function () { return "SW"; });
patch(String.prototype, "toUpperCase", function () { return "U"; });
patch(String.prototype, "substring", function () { return "SUB"; });
patch(Object.prototype, "hasOwnProperty", function () { return "H"; });
for (let i = 0; i < 50; i++) round();
note("patched  " + round());
note("identity " + cat([" z ".trim === String.prototype.trim, [].join === Array.prototype.join,
                        (1).toFixed === Number.prototype.toFixed, new Map().get === Map.prototype.get,
                        /x/.test === RegExp.prototype.test, ({}).hasOwnProperty === Object.prototype.hasOwnProperty], ","));

for (const name of ["trim", "split", "indexOf", "startsWith", "toUpperCase", "substring"]) String.prototype[name] = saved[name];
for (const name of ["join", "slice", "push", "map", "sort"]) Array.prototype[name] = saved[name];
Number.prototype.toFixed = saved.toFixed;
Map.prototype.get = saved.get;
RegExp.prototype.test = saved.test;
Object.prototype.hasOwnProperty = saved.hasOwnProperty;
for (let i = 0; i < 50; i++) round();
note("restored " + round());

// A new member on a kind's prototype (a polyfill) is reached, and leaves the others alone.
String.prototype.shout = function () { return this.toUpperCase() + "!"; };
Number.prototype.double = function () { return this * 2; };
Boolean.prototype.flip = function () { return !this.valueOf(); };
note("added    " + cat(["hi".shout(), (4).double(), true.flip(), trims(" q "), joins([3, 4])], "|"));

// Primitives inherit Object.prototype.
note("inherit  " + cat([typeof "x".isPrototypeOf, typeof (1).propertyIsEnumerable, typeof true.hasOwnProperty,
                        typeof "x".__defineGetter__ === typeof ({}).__defineGetter__], ","));
for (let i = 0; i < log.length; i++) console.log(log[i]);
