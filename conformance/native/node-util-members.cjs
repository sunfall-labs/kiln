// What `require("node:util")` publishes, and what functions answer.
const util = require("node:util");

const names = Object.getOwnPropertyNames(util).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof util[name]);
}

console.log("default self-link", util.default === util);

// format
console.log("fmt1:", util.format("%s:%d %%", "port", 3000));
console.log("fmt2:", util.format("hello", "world", 42));
console.log("fmt3:", util.format("%j", { key: "val" }));

// inherits
function Vehicle() {}
Vehicle.prototype.drive = function() { return "vroom"; };
function Car() {}
util.inherits(Car, Vehicle);
const car = new Car();
console.log("inherits drive:", car.drive());
console.log("inherits super_:", Car.super_ === Vehicle);
console.log("inherits proto link:", Object.getPrototypeOf(Car.prototype) === Vehicle.prototype);

// debuglog
const log = util.debuglog("test");
console.log("debuglog is fn:", typeof log === "function");

// deprecate
const orig = () => 42;
const dep = util.deprecate(orig, "deprecated function");
console.log("deprecate call:", dep());

// promisify
const cbFn = (val, cb) => cb(null, val * 2);
const promiseFn = util.promisify(cbFn);
promiseFn(21).then((res) => {
  console.log("promisify res:", res);
});

// types
console.log("types.isPromise:", util.types.isPromise(Promise.resolve(1)));
console.log("types.isDate:", util.types.isDate(new Date()));
console.log("types.isArrayBuffer:", util.types.isArrayBuffer(new ArrayBuffer(8)));

// TextDecoder and TextEncoder
console.log("TextDecoder is fn:", typeof util.TextDecoder === "function");
console.log("TextEncoder is fn:", typeof util.TextEncoder === "function");
