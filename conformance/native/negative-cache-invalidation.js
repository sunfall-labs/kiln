// A read that misses may be remembered by its site ("this shape has no such key, up its chain"),
// and every case below changes the answer *after* the site has learned the miss. Each reader is
// one function, so every read below goes through one compiled site; the warm-up loop is what
// makes the site learn. The interpreter is the oracle.

function warm(read, value) {
  let seen = 0;
  for (let i = 0; i < 50; i++) if (read(value) === undefined) seen++;
  return seen;
}

// 1. A property added to Object.prototype after the miss was learned.
{
  const readX = (o) => o.lateOnObjectProto;
  const o = { a: 1, b: 2 };
  console.log("object-proto warm", warm(readX, o));
  Object.prototype.lateOnObjectProto = "from Object.prototype";
  console.log("object-proto after", readX(o), readX({ a: 3, b: 4 }));
  // Not deleted again: `delete` on Object.prototype's program-written store is a separate native
  // gap (the member stays), and a later case must not inherit it.
  Object.prototype.lateOnObjectProto = undefined;
  console.log("object-proto undefined again", readX(o));
}

// 2. A getter, then a setter-only accessor, defined on the receiver's prototype after the miss.
{
  const readG = (o) => o.lateGetter;
  const proto = { base: true };
  const o = Object.create(proto);
  o.a = 1;
  console.log("getter warm", warm(readG, o));
  Object.defineProperty(proto, "lateGetter", {
    get() { return "getter:" + this.a; },
    configurable: true,
  });
  console.log("getter after", readG(o));
  delete proto.lateGetter;
  console.log("getter deleted", readG(o));
  Object.defineProperty(proto, "lateGetter", { set(v) {}, configurable: true });
  console.log("setter only", readG(o), "lateGetter" in o);
}

// 3. The same shape reached from a different prototype: a shape says nothing about the chain.
{
  const readY = (o) => o.y;
  const plain = { a: 1 };
  console.log("same-shape warm", warm(readY, plain));
  const withProto = Object.create({ y: "inherited y" });
  withProto.a = 1;
  console.log("same-shape other proto", readY(withProto), readY(plain));
}

// 4. The receiver relinked with __proto__ and with Object.setPrototypeOf.
{
  const readZ = (o) => o.z;
  const o = { a: 1, c: 2 };
  console.log("relink warm", warm(readZ, o));
  o.__proto__ = { z: "via __proto__" };
  console.log("relink __proto__", readZ(o));
  const p = { a: 1, c: 2 };
  warm(readZ, p);
  Object.setPrototypeOf(p, { z: "via setPrototypeOf" });
  console.log("relink setPrototypeOf", readZ(p));
}

// 5. A property added to an object on the chain, and to a class prototype.
{
  const proto = { base: 1 };
  const readW = (o) => o.w;
  const o = Object.create(proto);
  o.a = 1;
  console.log("chain warm", warm(readW, o));
  proto.w = "added to proto";
  console.log("chain after", readW(o));
  class K { constructor() { this.a = 1; } }
  const readM = (o) => o.m;
  const k = new K();
  console.log("class warm", warm(readM, k));
  K.prototype.m = "added to class prototype";
  console.log("class after", readM(k));
  Object.defineProperty(K.prototype, "n", { get() { return "class getter"; }, configurable: true });
  const readN = (o) => o.n;
  console.log("class getter", readN(k));
}

// 6. A property added to the receiver itself (a new shape) and to a sibling of one shape.
{
  const readV = (o) => o.v;
  const o = { a: 1 };
  console.log("own warm", warm(readV, o));
  o.v = "own v";
  console.log("own after", readV(o));
  const q = { a: 2 };
  Object.defineProperty(q, "v", { value: "defined v", enumerable: false });
  console.log("own defined", readV(q));
}

// 7. Exotic receivers at a site that has learned a miss on a plain object.
{
  const readL = (o) => o.length;
  const read0 = (o) => o[0];
  const plain = { a: 1 };
  console.log("exotic warm", warm(readL, plain), warm(read0, plain));
  console.log("exotic array", readL([1, 2, 3]), read0([7]));
  console.log("exotic string", readL("abcd"), read0("xy"));
  console.log("exotic typed", readL(new Uint8Array(5)), read0(new Uint8Array([9])));
  const indexed = { a: 1 };
  indexed[0] = "zero";
  console.log("exotic indexed object", read0(indexed));
  console.log("exotic arguments", (function () { return readL(arguments); })(1, 2));
}

// 8. A Proxy receiver and a module-namespace-like frozen object at a warmed site.
{
  const readP = (o) => o.p;
  const plain = { a: 1 };
  console.log("proxy warm", warm(readP, plain));
  const proxy = new Proxy({ a: 1 }, { get: (t, k) => (k === "p" ? "trapped" : t[k]) });
  console.log("proxy after", readP(proxy));
  const frozen = Object.freeze({ a: 1 });
  console.log("frozen", readP(frozen));
  const nullProto = Object.create(null);
  nullProto.a = 1;
  console.log("null proto", readP(nullProto), warm(readP, nullProto));
}

// 9. A dictionary-mode object (many deletes) and a wide object past the fast-property limit.
{
  const readD = (o) => o.d;
  const dict = { a: 1, b: 2, c: 3 };
  delete dict.b;
  console.log("dict warm", warm(readD, dict));
  dict.d = "dict d";
  console.log("dict after", readD(dict));
  const wide = {};
  for (let i = 0; i < 40; i++) wide["k" + i] = i;
  console.log("wide warm", warm(readD, wide));
  wide.d = "wide d";
  console.log("wide after", readD(wide));
}

// 10. A method added to Object.prototype and read as a call at a warmed site.
{
  const call = (o) => (typeof o.lateMethod === "function" ? o.lateMethod() : "absent");
  const o = { a: 1 };
  for (let i = 0; i < 50; i++) call(o);
  console.log("method before", call(o));
  Object.prototype.lateMethod = function () { return "method:" + this.a; };
  console.log("method after", call(o));
  Object.prototype.lateMethod = undefined;
  console.log("method cleared", call(o));
}

// 11. Function.prototype changed after a miss on a function receiver.
{
  const readF = (o) => o.lateOnFunctionProto;
  function f() {}
  console.log("function warm", warm(readF, f));
  Function.prototype.lateOnFunctionProto = "fn proto";
  console.log("function after", readF(f));
}
