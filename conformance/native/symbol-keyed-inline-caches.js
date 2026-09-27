// Symbol-keyed reads, writes and definitions on shaped objects (realms issue 17).
//
// A symbol key is an ordinary shape key natively (its `@@` spelling), and `o[sym]` now takes the
// same shape, slot and prototype-path caches `o.name` does -- from a computed site whose entries
// are each learned for one key, because the site's key is whatever the program computes. So the
// cases here are the ones a key-blind cache would answer wrongly: one site reading several
// symbols off one shape, a site that learned an absent answer and then meets the key, a symbol
// found on the prototype that is then shadowed or replaced, a store that learned a transition
// meeting a setter or a frozen object, and well-known, registered and unique symbols through one
// site. Every case loops, so each site answers from its cache after the first pass.
//
// The last block is the enumerability half: a class method with a symbol key is not enumerable
// (it was, natively), and an object literal's symbol accessor is enumerable but never listed by
// the walks over string keys (it was listed as `@@toStringTag`).

"use strict";

const a = Symbol("a");
const b = Symbol("b");
const reg = Symbol.for("kiln.issue17");
const out = [];

// One site, several symbols, one shape.
const multi = { [a]: 1, [b]: 2, [reg]: 3, x: 4 };
function readKey(o, k) { return o[k]; }
for (let i = 0; i < 3; i++) out.push([a, b, reg].map((k) => readKey(multi, k)).join(","));

// One site, several shapes.
const shapes = [{ [a]: "p" }, { y: 0, [a]: "q" }, { z: 0, w: 0, [a]: "r" }];
for (let i = 0; i < 2; i++) out.push(shapes.map((o) => readKey(o, a)).join(""));

// Absent, then present: the absent path is proved again on every hit.
const later = {};
const answers = [];
for (let i = 0; i < 4; i++) {
  answers.push(String(readKey(later, b)));
  if (i === 1) later[b] = "now";
}
out.push(answers.join(","));

// Absent on the prototype, then the prototype gains it.
class Base {}
const inst = new Base();
const seen = [];
for (let i = 0; i < 4; i++) {
  seen.push(String(inst[a]));
  if (i === 1) Base.prototype[a] = "from-proto";
}
out.push(seen.join(","));

// Found on the prototype, then shadowed, then the prototype's replaced.
class Holder { [a]() { return "method"; } }
const h = new Holder();
const calls = [];
for (let i = 0; i < 6; i++) {
  calls.push(typeof h[a] === "function" ? h[a]() : h[a]);
  if (i === 1) h[a] = "own";
  if (i === 3) { delete h[a]; Holder.prototype[a] = function () { return "replaced"; }; }
}
out.push(calls.join(","));

// Stores: a learned transition, then a setter on the prototype for the same key.
function writeKey(o, k, v) { o[k] = v; return o; }
const written = [];
class WithSetter {}
for (let i = 0; i < 4; i++) {
  if (i === 2) {
    Object.defineProperty(WithSetter.prototype, b, {
      set(v) { written.push("setter:" + v); },
      get() { return "getter"; },
      configurable: true,
    });
  }
  const o = writeKey(new WithSetter(), b, i);
  written.push(Object.getOwnPropertySymbols(o).length + ":" + o[b]);
}
out.push(written.join(","));

// Stores into a frozen object, a non-extensible one and a read-only slot are refused (strict, so
// each refusal throws), after the same site has learned stores it may make.
const frozen = Object.freeze({ [a]: 1 });
const sealed = Object.preventExtensions({ [a]: 1 });
const readonly = {};
Object.defineProperty(readonly, a, { value: 1, writable: false, enumerable: true });
const refusals = [];
const tryWrite = (o, k, v) => {
  try {
    writeKey(o, k, v);
    refusals.push("ok");
  } catch (e) {
    refusals.push(e.constructor.name);
  }
};
for (let i = 0; i < 3; i++) {
  tryWrite({ [a]: 0 }, a, i);
  tryWrite(frozen, a, 9);
  tryWrite(sealed, b, 9);
  tryWrite(sealed, a, 5 + i);
  tryWrite(readonly, a, 9);
}
out.push(refusals.join(","));
out.push([frozen[a], sealed[a], String(sealed[b]), readonly[a]].join(","));

// A double-typed slot then a string.
const nums = { [a]: 1.5 };
for (let i = 0; i < 3; i++) writeKey(nums, a, i === 2 ? "text" : i + 0.5);
out.push(String(nums[a]));

// Getters are called with the receiver, own and inherited.
const withGetter = { get [a]() { return "g:" + this.tag; }, tag: "own" };
const child = Object.create(withGetter);
child.tag = "child";
for (let i = 0; i < 2; i++) out.push(readKey(withGetter, a) + "/" + readKey(child, a));

// Well-known symbols through the same site as unique ones.
class Iterable {
  *[Symbol.iterator]() { yield 1; yield 2; }
  get [Symbol.toStringTag]() { return "Iterable"; }
}
const it = new Iterable();
for (let i = 0; i < 2; i++) {
  out.push([typeof readKey(it, Symbol.iterator), readKey(it, Symbol.toStringTag), String(readKey({}, Symbol.iterator))].join(","));
}
out.push([...it].join("") + " " + Object.prototype.toString.call(it));

// Class fields keyed by symbols, then reads and writes in a loop (the Event shape).
class Ev {
  [a] = "";
  [b] = null;
  [reg] = false;
  #flags = 0;
  constructor(t) { this[a] = t; this.#flags = 1; }
  get type() { return this[a]; }
  flags() { return this.#flags; }
}
let evSum = "";
for (let i = 0; i < 5; i++) {
  const e = new Ev("t" + i);
  e[b] = i;
  e[reg] = !e[reg];
  evSum += e.type + e[b] + e[reg] + e.flags() + ";";
}
out.push(evSum);
out.push(Object.keys(new Ev("k")).length + " " + Object.getOwnPropertySymbols(new Ev("k")).length);

// Enumerability: symbol-keyed methods and class accessors are not enumerable.
const d = (o, k) => {
  const x = Object.getOwnPropertyDescriptor(o, k);
  return x === undefined ? "none" : [x.enumerable, x.writable, x.configurable].join("/");
};
class Methods {
  [a]() {}
  static [b]() {}
  [Symbol.iterator]() {}
  get [reg]() { return 1; }
}
out.push([d(Methods.prototype, a), d(Methods, b), d(Methods.prototype, Symbol.iterator), d(Methods.prototype, reg)].join(" "));
out.push(Object.getOwnPropertySymbols({ ...Methods.prototype }).length + " " +
  Object.getOwnPropertySymbols(Object.assign({}, Methods.prototype)).length);

// An object literal's symbol members are enumerable, and never string keys.
const literal = { [a]: 1, get [Symbol.toStringTag]() { return "L"; }, x: 2 };
const keys = [];
for (const k in literal) keys.push(k);
out.push([Object.keys(literal).join(","), keys.join(","), JSON.stringify(literal), String(literal),
  d(literal, a), d(literal, Symbol.toStringTag)].join(" "));
const copied = { ...literal };
const assigned = Object.assign({}, literal);
out.push([Object.getOwnPropertySymbols(copied).length, copied[a], copied[Symbol.toStringTag],
  Object.getOwnPropertySymbols(assigned).length, assigned[a], assigned[Symbol.toStringTag]].join(","));

// A symbol hidden with a descriptor stays hidden through spread and assign.
const hidden = { x: 1 };
Object.defineProperty(hidden, a, { value: 3, enumerable: false });
out.push(Object.getOwnPropertySymbols({ ...hidden }).length + " " +
  Object.getOwnPropertySymbols(Object.assign({}, hidden)).length);

console.log(out.join("\n"));
