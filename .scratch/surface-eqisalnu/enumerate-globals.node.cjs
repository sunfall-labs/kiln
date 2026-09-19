


function kilnDescribe(v) {
  var t = typeof v;
  if (t === "undefined") return "undefined";
  if (t === "object") return v === null ? "null" : "object";
  if (t === "function") return "function";
  if (t === "symbol") return "symbol";
  if (t === "string") return "string " + JSON.stringify(v);
  return t + " " + String(v);
}

var keys = [];
var vals = [];
function push(key, v) {
  if (v === null || (typeof v !== "object" && typeof v !== "function")) return;
  for (var i = 0; i < vals.length; i++) { if (vals[i] === v) return; }
  keys.push(key); vals.push(v);
}
function wellKnown(sym) {
  var names = Object.getOwnPropertyNames(Symbol);
  for (var i = 0; i < names.length; i++) {
    var candidate;
    try { candidate = Symbol[names[i]]; } catch (e) { continue; }
    if (typeof candidate === "symbol" && candidate === sym) return names[i];
  }
  return null;
}
function report(key, v) {
  var own;
  // The receiver's own answer, before any of its members. It is printed first because it is the
  // one reading that does not depend on the walk getting anywhere: a receiver whose own keys
  // cannot be listed still has a `typeof`, and `typeof require("node:module")` differing between
  // the engines went unmeasured for months precisely because nothing asked this.
  var selfName;
  try { selfName = kilnDescribe(v.name); } catch (e) { selfName = "throws"; }
  console.log("S\t" + key + "\t" + kilnDescribe(v) + "\t" + selfName);
  try { own = Object.getOwnPropertyNames(v); } catch (e) { return; }
  var syms = [];
  try { syms = Object.getOwnPropertySymbols(v); } catch (e) { syms = []; }
  for (var s = 0; s < syms.length; s++) {
    var wk = wellKnown(syms[s]);
    if (wk !== null) own.push("@@" + wk);
  }
  own = own.sort();
  for (var j = 0; j < own.length; j++) {
    var n = own[j];
    var mv;
    try {
      mv = n.indexOf("@@") === 0 ? v[Symbol[n.slice(2)]] : v[n];
    } catch (e) { console.log("M\t" + key + "\t" + n + "\tthrows"); continue; }
    console.log("M\t" + key + "\t" + n + "\t" + kilnDescribe(mv));
  }
}

var hostHook = "$262";
var skip = ["Storage", "localStorage", "sessionStorage"];
var globals = Object.getOwnPropertyNames(globalThis).filter(function (n) {
  return n.indexOf(hostHook) !== 0 && skip.indexOf(n) === -1;
}).sort();
push("globalThis", globalThis);
for (var i = 0; i < globals.length; i++) {
  try { push(globals[i], globalThis[globals[i]]); } catch (e) {}
}
for (var i2 = 0; i2 < globals.length; i2++) {
  var g = globals[i2];
  try {
    var c = globalThis[g];
    if (typeof c === "function") push(g + ".prototype", c.prototype);
  } catch (e) {}
}
var direct = keys.length;
for (var i3 = 0; i3 < direct; i3++) {
  try { push(keys[i3] + "^", Object.getPrototypeOf(vals[i3])); } catch (e) {}
}
for (var i4 = 0; i4 < keys.length; i4++) report(keys[i4], vals[i4]);
console.log("#END");
