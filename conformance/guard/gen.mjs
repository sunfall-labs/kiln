import fs from 'node:fs';
// one probe per behaviour this stack claims to fix. each must match node exactly.
const P = {
'g01-spread-symbols': `const s = Symbol('s');
const src = { a: 1, [s]: 2, [Symbol.iterator]: 3 };
const { a, ...rest } = src;
console.log(({ ...src })[s], rest[Symbol.iterator], Object.keys(src).join(','));`,

'g02-arrow-arguments': `function f() { return () => arguments.length }
function g() { return [1,2].map(() => arguments.length) }
console.log(f(1,2)(), g('a','b','c').join(','));`,

'g03-class-field-closure': `const mk = (tag) => { class B { _tag = tag; f = () => tag } return B };
const b = new (mk('T'))();
console.log(b._tag, b.f());`,

'g04-generic-forof': `const o = { [Symbol.iterator]() { let i = 0; return { next: () => i < 3 ? { value: i++, done: false } : { done: true } } } };
let s = 0; for (const x of o) s += x;
console.log(s, [...o].join(','));`,

'g05-yieldstar-protocol': `class SSG { constructor(self) { this.self = self; this.called = false }
  next(a) { return this.called ? { value: a, done: true } : (this.called = true, { value: this.self, done: false }) }
  [Symbol.iterator]() { return new SSG(this.self) } }
class Eff { constructor(op) { this.op = op } [Symbol.iterator]() { return new SSG(this) } }
function* p() { const a = yield* new Eff('one'); const b = yield* new Eff('two'); return a + b }
const it = p(); let step = it.next(); const outs = [];
while (!step.done) { outs.push(step.value.op); step = it.next(step.value.op.toUpperCase()) }
console.log(outs.join(','), step.value);`,

// deliberately references nothing else: a program whose ONLY use is a symbol key must still
// find the prototype property, which is the conditional case that hid a defect
'g06-array-proto-iterator': `console.log(typeof [1,2][Symbol.iterator]);`,

'g06b-array-iterator-identity': `console.log(Symbol.iterator in [1,2], [][Symbol.iterator] === [].values);`,

'g07-timers-order': `console.log('a');
const t = setTimeout(() => console.log('never'), 5); clearTimeout(t);
setTimeout((x) => console.log('args', x), 1, 'p');
setTimeout(() => console.log('late'), 20);
let n = 0; const iv = setInterval(() => { n++; if (n === 2) clearInterval(iv); console.log('iv', n) }, 2);
queueMicrotask(() => console.log('micro'));
console.log('b');`,

'g08-finally-paths': `function a() { try { return 'A' } finally { console.log('finA') } } console.log(a());
function b() { try { throw new Error('B') } finally { console.log('finB') } }
try { b() } catch (e) { console.log('caught', e.message) }
function c() { for (let i = 0; i < 3; i++) { try { if (i === 1) continue; if (i === 2) break } finally { console.log('fin', i) } } return 'done' }
console.log(c());`,

'g09-error-subclass': `class E extends Error { constructor(m) { super(m); this.name = 'E'; this.tag = 'T' } }
const e = new E('boom');
console.log(e instanceof Error, e instanceof E, e.message, e.name, e.tag, String(e));
class F extends globalThis.Error {}
console.log(new F('g') instanceof Error, new F('g').message);`,

'g10-builtin-statics': `const A = Array, O = Object, J = JSON;
console.log(typeof A.isArray, typeof A.from, typeof O.keys, typeof J.parse);
console.log(A.isArray([1]), O.keys({ x: 1 }).join(','));`,

'g11-proto-on-function': `const proto = { [Symbol.iterator]() { return { next: () => ({ done: true }) } }, foo() { return 'f' }, tag: 'T' };
function K() {}
Object.setPrototypeOf(K, proto);
class Sub extends K {}
console.log(typeof K[Symbol.iterator], K.foo(), K.tag, typeof Sub[Symbol.iterator], Sub.tag);`,

'g12-private-not-inherited': `class C { static f() { return this.#g() } static #g() { return 42 } }
class D extends C {}
console.log(C.f());
try { D.f(); console.log('no throw') } catch (e) { console.log('threw', e.constructor.name) }
class A { #m() { return 1 } run() { return this.#m() } }
class B extends A {}
console.log('inherited instance private:', new B().run());`,

'g13-set-mutation': `const s = new Set();
const a = { n: 'a' }, b = { n: 'b' }, c = { n: 'c' };
s.add(a); s.add(b); s.add(c);
const seen = [];
for (const x of s) { seen.push(x.n); s.delete(x) }
console.log(seen.length, seen.join(','), s.size);`,

'g14-generator-return-finally': `function* g() { try { yield 1; yield 2 } finally { console.log('cleanup') } }
const it = g(); console.log(it.next().value);
console.log(JSON.stringify(it.return('early')));`,

// stack is header-only by design: the release build strips unwind tables, so real frames
// are not obtainable. Effect only needs the property to exist and be a string
'g15-error-stack-cause': `const inner = new Error('inner');
const e = new Error('outer', { cause: inner });
console.log(typeof e.stack, e.cause === inner, typeof Error.captureStackTrace);`,

// destructuring a generic iterable is deliberately NOT in scope: the fix trades a loud
// TypeError for a hang on an infinite iterable, and Effect does not need it
'g16-array-from-iterable': `const c = { *[Symbol.iterator]() { yield 1; yield 2 } };
console.log(Array.from(c).join(','), [...c].join(','));`,

'g17-set-map-iterator': `console.log(typeof new Set()[Symbol.iterator], typeof new Map()[Symbol.iterator], typeof 'ab'[Symbol.iterator]);
function* g() { yield 1 }
console.log(typeof g()[Symbol.iterator]);`,

'g18-iterator-close': `const log = [];
const it = { [Symbol.iterator]() { return { next: () => ({ value: 1, done: false }), return() { log.push('closed'); return { done: true } } } } };
for (const x of it) break;
console.log(log.join(',') || 'not closed');`,

'g19-subclass-array-map': `class A extends Array {}
const a = new A(); a.push(1);
console.log(a instanceof Array, a.length);
class M extends Map {}
const m = new M(); m.set('k', 1);
console.log(m instanceof Map, m.get('k'));`
};
for (const [k, v] of Object.entries(P)) fs.writeFileSync(k + '.js', v + '\n');
console.log('wrote', Object.keys(P).length, 'guard probes');
