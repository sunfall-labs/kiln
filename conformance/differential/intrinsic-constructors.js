// Every intrinsic value names the built-in that makes it, which is how a library clones a value
// whose type it was handed rather than told.
console.log(new Date(0).constructor === Date);
console.log(/x/.constructor === RegExp, [].constructor === Array);
console.log("s".constructor === String, (1).constructor === Number);
console.log(true.constructor === Boolean, Promise.resolve().constructor === Promise);
console.log(new (new Date(5)).constructor(7).getTime());

// A prototype table names its own constructor.
console.log(Object.prototype.constructor === Object);
console.log(Array.prototype.constructor === Array);

// The realm has one global object, and something stored on it is found again.
globalThis.__probe = { count: 0 };
globalThis.__probe.count += 1;
console.log(globalThis === globalThis, globalThis.__probe.count);
let holder;
(holder = globalThis).__lazy ?? (holder.__lazy = "made");
console.log(globalThis.__lazy);

// A regexp match carries where it matched and what it matched against.
const m = "hello".match(/l+/);
console.log(m[0], m.index, m.input, m.groups);
console.log("hello".match(/z/));
