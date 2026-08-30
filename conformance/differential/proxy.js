// Every trap a program ordinarily writes, and the fall-through when one is absent.
const target = { a: 1, b: 2 };
const log = [];
const p = new Proxy(target, {
  get(t, k) { log.push("get:" + String(k)); return t[k]; },
  set(t, k, v) { log.push("set:" + String(k)); t[k] = v; return true; },
  has(t, k) { log.push("has:" + String(k)); return k in t; },
  deleteProperty(t, k) { log.push("del:" + String(k)); delete t[k]; return true; },
  ownKeys(t) { log.push("keys"); return Reflect.ownKeys(t); },
});
console.log(p.a);
p.c = 3;
console.log("b" in p, "z" in p);
delete p.b;
console.log(Object.keys(p).join(","), JSON.stringify(target));
console.log(log.join(" "));

// A handler with no traps at all passes everything through.
const bare = new Proxy({ x: 9 }, {});
console.log(bare.x, "x" in bare, Object.keys(bare).join(","), JSON.stringify(bare));

// A symbol key stays a symbol, which is how a library hides its own state behind a proxy.
const secret = Symbol("secret");
const keyed = new Proxy({}, {
  get(t, k) { return k === secret ? "hidden" : "plain"; },
});
console.log(keyed[secret], keyed.anything);

// Callable and constructible proxies.
const fn = new Proxy((a, b) => a + b, { apply(t, self, args) { return t(...args) * 10; } });
console.log(fn(1, 2), fn.call(null, 3, 4));
class Point { constructor(v) { this.v = v; } }
const Ctor = new Proxy(Point, { construct(t, args) { return new t(args[0] * 2); } });
console.log(new Ctor(4).v, new Ctor(1) instanceof Point);

// A proxy of an array is an array, and a mutating method writes through the trap.
const writes = [];
const list = new Proxy([1, 2, 3], {
  set(t, k, v) { writes.push(String(k)); t[k] = v; return true; },
});
console.log(Array.isArray(list), list.length);
list.push(4);
console.log(list.length, JSON.stringify(list), writes.length > 0);

// A revoked proxy refuses everything.
const { proxy, revoke } = Proxy.revocable({ q: 1 }, {});
console.log(proxy.q);
revoke();
try { proxy.q; } catch (error) { console.log("revoked", error instanceof TypeError); }

// for-in goes through ownKeys.
const enumerated = [];
for (const key in new Proxy({ m: 1, n: 2 }, {})) enumerated.push(key);
console.log(enumerated.join(","));

// Describing and defining a property are traps of their own, and a copy-on-write layer goes
// through exactly this pair.
const described = [];
const shaped = new Proxy({ k: 1 }, {
  getOwnPropertyDescriptor(t, k) {
    described.push(String(k));
    return Object.getOwnPropertyDescriptor(t, k);
  },
  defineProperty(t, k, d) {
    Object.defineProperty(t, k, d);
    return true;
  },
});
console.log(Object.getOwnPropertyDescriptor(shaped, "k").value);
Object.defineProperty(shaped, "j", { value: 2, enumerable: true, configurable: true });
console.log(shaped.j, described.join(","));

// `hasOwnProperty` asks the same question and must not read a store the proxy does not have.
console.log(Object.prototype.hasOwnProperty.call(shaped, "k"));
console.log(Object.prototype.hasOwnProperty.call(shaped, "absent"));
console.log(Object.hasOwn(shaped, "j"));
