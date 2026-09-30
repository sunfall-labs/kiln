class A extends Array {}
const a = A.from([1, 2], x => x * 3);
console.log(a instanceof A, Array.isArray(a), a.join(","), a.map(x => x) instanceof A);
const b = A.from({0: 4, 1: 5, length: 2});
console.log(b instanceof A, b.join(","));
const order = [];
function C(length) { order.push("construct:" + arguments.length + ":" + length); }
const source = {get length() { order.push("length"); return 2; }, get 0() {order.push("zero"); return 7;}, 1: 8};
const c = Array.from.call(C, source, x => { order.push("map:" + x); return x; });
console.log(c instanceof C, c[0], c[1], c.length, order.join(","));
order.length = 0;
const iterable = { [Symbol.iterator]() {order.push("iterate"); let n = 0; return {next() {return n++ ? {done: true} : {value: 9, done: false};}};}};
const i = Array.from.call(C, iterable);
console.log(i instanceof C, i[0], i.length, order.join(","));
let closed = false;
function Locked() { Object.preventExtensions(this); }
const closable = {[Symbol.iterator]() {let n = 0; return {next() {return n++ < 2 ? {done: false, value: 1} : {done: true};}, return() {closed = true; return {};}};}};
try { Array.from.call(Locked, closable); } catch (e) {console.log(e.name, closed);}
const ordinary = Array.from.call(Object, [1, 2]);
console.log(Array.isArray(ordinary), ordinary[0], ordinary[1], ordinary.length);
console.log(Array.from.call(Math, [1, 2]).join(","));

function Plain() {}
a.constructor = Plain;
const plain = a.map(x => x);
console.log(Array.isArray(plain), plain instanceof Plain, Plain[Symbol.species] === undefined);
let speciesReceiver = false;
Object.defineProperty(Plain, Symbol.species, {get() {speciesReceiver = this === Plain; return C;}, configurable: true});
const custom = a.map(x => x + 1);
console.log(speciesReceiver, custom instanceof C, custom[0], custom[1]);
Object.defineProperty(Plain, Symbol.species, {value: null});
console.log(Array.isArray(a.map(x => x)));
