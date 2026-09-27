// `{ m() {} }.m` is how a built-in is written to be a method: no `prototype`, no [[Construct]].
// The compiler reads the method straight out of the definition when nothing could observe the
// literal (realms issue 08) and keeps the literal when `super` or a direct `eval` could reach it
// through the method's [[HomeObject]] (the direct-`eval` case is a kiln-ir test: the native
// backend refuses a program with one). Every answer here is node's.
const plain = { getTime(x, y) { return 7; } }.getTime;
console.log(plain.name, plain.length, typeof plain, "prototype" in plain, plain());
try { new plain(); console.log("constructed"); } catch (e) { console.log(e.constructor.name); }
console.log(plain.toString());
const gen = { *gen() { yield 1; yield 2; } }.gen;
console.log(gen.name, "prototype" in gen, [...gen()].join());
const asyncMethod = { async am() { return 2; } }.am;
console.log(asyncMethod.name);
asyncMethod().then((v) => console.log("async", v));
const quoted = { "two words"() { return "q"; } }["two words"];
console.log(quoted.name, quoted());
const withSuper = { m() { return super.toString === Object.prototype.toString; } }.m;
console.log("super", withSuper());
const arrowSuper = { m() { return (() => super.hasOwnProperty)() === Object.prototype.hasOwnProperty; } }.m;
console.log("arrow super", arrowSuper());
const other = { m() { return 1; } }.n;
console.log("other key", other);
const a = { m() { return 1; } }.m;
const b = { m() { return 1; } }.m;
console.log("distinct", a !== b, a.name === b.name);
let made = 0;
function make() { made += 1; return { run() { return made; } }.run; }
console.log(make()(), make()(), make() !== make());
