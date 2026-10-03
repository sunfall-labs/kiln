const tag = (value) => Object.prototype.toString.call(value);
for (const target of [{}, [], function fn() {}, /x/, new Error('x'), new Map(), new Set()]) {
  console.log(tag(new Proxy(target, {})));
}
console.log(tag(new Proxy(new Proxy([], {}), {})));
console.log(tag(new Proxy({}, { get(target, key, receiver) {
  return key === Symbol.toStringTag ? 'Custom' : Reflect.get(target, key, receiver);
} })));
console.log(tag(new Proxy({ [Symbol.toStringTag]: 42 }, {})));
let gets = 0;
const inner = Proxy.revocable([], {});
const outer = new Proxy(inner.proxy, { get(target, key, receiver) {
  gets++;
  return Reflect.get(target, key, receiver);
} });
inner.revoke();
try { tag(outer); } catch (error) { console.log(error instanceof TypeError, gets); }
const boom = new Error('getter');
try { tag(new Proxy({}, { get() { throw boom; } })); }
catch (error) { console.log(error === boom); }
console.log(/x/[Symbol.toStringTag], RegExp.prototype[Symbol.toStringTag]);
