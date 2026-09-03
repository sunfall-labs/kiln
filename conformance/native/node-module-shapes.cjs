// What `require("node:x")` hands back, for the four modules whose answer was not the same in the
// two engines.
//
// Each line is `typeof`, then the name if it is callable, then a shape reading that proves the
// value is the thing it claims to be rather than a placeholder that happens to type-check.
//
// WHAT WAS WRONG HERE. `require("node:assert")` and `require("node:events")` are node's callable
// defaults, and node names them for the module: "assert" and "EventEmitter". The interpreter
// answered "nodeAssertOk" and "nodeEventEmitter" -- its own Rust identifiers -- and the native
// backend answered the empty string, so the two engines disagreed *and* both were wrong.
// `http.METHODS` is a constant array; the native backend published a stand-in callable for it, so
// `typeof http.METHODS` was "function" against the interpreter's "object". And `performance` is a
// `Performance` object rather than a function, which the interpreter called "function" -- while
// the native backend built `require("node:perf_hooks").performance` as a *second* object, so
// `perf.performance === globalThis.performance` was false where node says true. A library reads
// that identity to decide whether it has already installed its own timing shim.
//
// `require` is spelled out once per module on purpose: a computed specifier is not a static
// import, and the native backend cannot resolve one.
function row(label, value) {
  console.log(
    label,
    typeof value,
    typeof value === "function" ? JSON.stringify(value.name) : "-",
  );
}

const assert = require("node:assert");
const events = require("node:events");
const http = require("node:http");
const perf = require("node:perf_hooks");

row("node:assert", assert);
row("node:events", events);
row("node:http", http);
row("node:perf_hooks", perf);

// `assert` really asserts, and `EventEmitter` really constructs one: a name on a value that does
// nothing would be a prettier wrong answer, not a right one.
assert(true);
const emitter = new events();
let heard = "";
emitter.on("ping", (value) => {
  heard = value;
});
emitter.emit("ping", "pong");
console.log("emitter heard", heard);

console.log("METHODS", Array.isArray(http.METHODS), http.METHODS.length);
console.log("METHODS join", http.METHODS.join(","));

console.log("performance is one object", perf.performance === globalThis.performance);
console.log("performance.now", typeof perf.performance.now());
console.log("performance.name", perf.performance.name);
// `typeof` is not the only conversion that has to agree: an object that stringifies as a function
// is still telling a program it is callable, and `String(x)` is where the other half of the
// `/[native code]/` feature test looks.
console.log("performance String", String(perf.performance));
console.log("performance tag", Object.prototype.toString.call(perf.performance));
console.log(
  "performance inherits Object.prototype",
  Object.getPrototypeOf(perf.performance) === Object.prototype,
);

// `require("node:module")` and `require("node:stream")` are node's other two callable defaults:
// the `Module` and `Stream` classes, `typeof` "function" in node v25.9.0 and in the interpreter.
// The native backend built a namespace object for both, so the same bytecode read "object" here
// and "function" there -- and `typeof require("node:module") === "function"` is how a bundle
// decides it may call `createRequire` off it.
//
// Only `typeof` is printed, and the omission is the point: `.name` is "Module" and "Stream" in
// node, `undefined` in the interpreter and the empty string here, so printing it would assert a
// disagreement neither engine has closed rather than the one this probe exists for. The members
// below are printed instead, because a callable that answers nothing would type-check and still
// be a placeholder.
const nodeModule = require("node:module");
const stream = require("node:stream");

console.log("node:module", typeof nodeModule, typeof nodeModule.createRequire);
console.log(
  "node:stream",
  typeof stream,
  typeof stream.Readable,
  typeof stream.Writable,
  typeof stream.addAbortSignal,
  typeof stream.finished,
);

// The class really constructs one: `new Readable()` off the callable default is the read a stream
// consumer makes, and a name on a value that does nothing would be a prettier wrong answer.
const readable = new stream.Readable();
readable.push("chunk");
console.log("readable read", readable.read());
