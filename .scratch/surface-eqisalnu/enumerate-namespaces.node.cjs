


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

try { push("node:assert", require("node:assert")); } catch (e) {}
try { push("node:async_hooks", require("node:async_hooks")); } catch (e) {}
try { push("node:buffer", require("node:buffer")); } catch (e) {}
try { push("node:crypto", require("node:crypto")); } catch (e) {}
try { push("node:diagnostics_channel", require("node:diagnostics_channel")); } catch (e) {}
try { push("node:dns", require("node:dns")); } catch (e) {}
try { push("node:events", require("node:events")); } catch (e) {}
try { push("node:fs", require("node:fs")); } catch (e) {}
try { push("node:fs/promises", require("node:fs/promises")); } catch (e) {}
try { push("node:http", require("node:http")); } catch (e) {}
try { push("node:http2", require("node:http2")); } catch (e) {}
try { push("node:https", require("node:https")); } catch (e) {}
try { push("node:module", require("node:module")); } catch (e) {}
try { push("node:net", require("node:net")); } catch (e) {}
try { push("node:os", require("node:os")); } catch (e) {}
try { push("node:path", require("node:path")); } catch (e) {}
try { push("node:perf_hooks", require("node:perf_hooks")); } catch (e) {}
try { push("node:querystring", require("node:querystring")); } catch (e) {}
try { push("node:stream", require("node:stream")); } catch (e) {}
try { push("node:string_decoder", require("node:string_decoder")); } catch (e) {}
try { push("node:tty", require("node:tty")); } catch (e) {}
try { push("node:url", require("node:url")); } catch (e) {}
try { push("node:util", require("node:util")); } catch (e) {}
try { push("node:worker_threads", require("node:worker_threads")); } catch (e) {}
try { push("node:zlib", require("node:zlib")); } catch (e) {}
for (var i = 0; i < keys.length; i++) report(keys[i], vals[i]);
console.log("#END");
