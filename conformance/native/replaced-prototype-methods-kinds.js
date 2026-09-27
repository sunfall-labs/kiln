// Parallel-builtins issue 30, item 1: every kind whose instances answer their prototype's
// members from a method table -- a regexp, a date, a promise, a set, a function, a symbol, a
// bigint, a typed array -- reaches the member the program wrote there instead. (The interpreter
// answered `re.test` and `promise.then` ahead of the prototype's store, so it disagreed with node
// too.) A member written on the prototype and then deleted leaves the table's in view again.
const r = /a/;
RegExp.prototype.test = function () { return "RT"; };
RegExp.prototype.exec = function () { return "EX"; };
console.log(r.test("a"), /a/.test === RegExp.prototype.test, r.exec("a"));
Date.prototype.getTime = function () { return "GT"; };
console.log(new Date(0).getTime());
Promise.prototype.then = function () { return "TH"; };
console.log(Promise.resolve(1).then());
Set.prototype.has = function () { return "SH"; };
console.log(new Set().has(1));
const call = Function.prototype.call;
Function.prototype.call = function () { return "FC"; };
console.log((function () { return 1; }).call(null));
Function.prototype.call = call;
Symbol.prototype.toString = function () { return "ST"; };
console.log(Symbol("q").toString());
BigInt.prototype.toString = function () { return "BT"; };
console.log((1n).toString());
Uint8Array.prototype.fill = function () { return "UF"; };
console.log(new Uint8Array(2).fill(1));
Float64Array.prototype.map = function () { return "patched"; };
console.log(new Float64Array(1).map((x) => x + 1), delete Float64Array.prototype.map,
            new Float64Array([1]).map((x) => x + 1)[0]);
String.prototype.zz = 1;
console.log("a".zz, delete String.prototype.zz, "a".zz);
