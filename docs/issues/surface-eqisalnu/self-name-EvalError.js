

function kilnDescribe(v) {
  var t = typeof v;
  if (t === "undefined") return "undefined";
  if (t === "object") return v === null ? "null" : "object";
  if (t === "function") return "function";
  if (t === "symbol") return "symbol";
  if (t === "string") return "string " + JSON.stringify(v);
  return t + " " + String(v);
}

var recv;
try { recv = globalThis.EvalError; } catch (e) { recv = undefined; }
console.log("T\t" + kilnDescribe(recv));
console.log("N\t" + kilnDescribe(recv.name));
console.log("#END");
