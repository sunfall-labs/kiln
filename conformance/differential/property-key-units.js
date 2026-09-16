"use strict";

// Keep keys as UTF-16 code units in the oracle output. String() and JSON.stringify() are
// deliberately absent from the key observations because both can hide a lone surrogate.
const units = (text) => {
  const result = [];
  for (let index = 0; index < text.length; index += 1) result.push(text.charCodeAt(index));
  return result;
};
const ownKeyUnits = (object) => Reflect.ownKeys(object).map((key) => units(key));
const lone = "\ud800";

const literal = { [lone]: 11 };
console.log("literal", JSON.stringify(ownKeyUnits(literal)), literal[lone]);

const defined = {};
Reflect.defineProperty(defined, lone, {
  value: 22,
  writable: true,
  enumerable: true,
  configurable: true,
});
const descriptor = Object.getOwnPropertyDescriptor(defined, lone);
console.log(
  "define-descriptor",
  JSON.stringify(ownKeyUnits(defined)),
  descriptor.value,
  descriptor.writable,
  descriptor.enumerable,
  descriptor.configurable,
);
console.log("object-keys", JSON.stringify(Object.keys(defined).map(units)));
console.log("reflect-ownKeys", JSON.stringify(Reflect.ownKeys(defined).map(units)));
console.log("in", lone in defined, Object.prototype.hasOwnProperty.call(defined, lone));

let inheritedGets = 0;
let inheritedSets = 0;
const prototype = {};
Object.defineProperty(prototype, lone, {
  get() {
    inheritedGets += 1;
    return 31;
  },
  set() {
    inheritedSets += 1;
  },
  enumerable: true,
  configurable: true,
});
const inherited = Object.create(prototype);
console.log("inherited-get", inherited[lone], lone in inherited, inheritedGets, inheritedSets);
inherited[lone] = 32;
console.log(
  "inherited-set",
  inheritedGets,
  inheritedSets,
  Object.prototype.hasOwnProperty.call(inherited, lone),
);

const lockedTarget = {};
Object.defineProperty(lockedTarget, lone, {
  value: 41,
  writable: false,
  enumerable: true,
  configurable: false,
});
const proxyEvents = [];
const proxy = new Proxy(lockedTarget, {
  get(target, key, receiver) {
    proxyEvents.push(["get", units(key)]);
    return Reflect.get(target, key, receiver);
  },
  set(target, key, value, receiver) {
    proxyEvents.push(["set", units(key)]);
    return Reflect.set(target, key, value, receiver);
  },
  deleteProperty(target, key) {
    proxyEvents.push(["delete", units(key)]);
    return Reflect.deleteProperty(target, key);
  },
  ownKeys(target) {
    proxyEvents.push(["ownKeys"]);
    return Reflect.ownKeys(target);
  },
});
console.log("proxy-get", proxy[lone], JSON.stringify(proxyEvents));
proxyEvents.length = 0;
try {
  proxy[lone] = 42;
} catch (error) {
  console.log("proxy-set", error.name, JSON.stringify(proxyEvents));
}
proxyEvents.length = 0;
try {
  delete proxy[lone];
} catch (error) {
  console.log("proxy-delete", error.name, JSON.stringify(proxyEvents));
}
proxyEvents.length = 0;
console.log("proxy-ownKeys", JSON.stringify(ownKeyUnits(proxy)), JSON.stringify(proxyEvents));

const frozen = Object.freeze({ [lone]: 51 });
try {
  frozen[lone] = 52;
} catch (error) {
  console.log("frozen-write", error.name, frozen[lone]);
}
const nonWritable = {};
Object.defineProperty(nonWritable, lone, {
  value: 61,
  writable: false,
  enumerable: true,
  configurable: true,
});
try {
  nonWritable[lone] = 62;
} catch (error) {
  console.log("nonwritable-write", error.name, nonWritable[lone]);
}

let coercions = 0;
const coercingKey = {
  toString() {
    coercions += 1;
    return lone;
  },
};
const coerced = {};
coerced[coercingKey] = 71;
console.log("coercion-write", coercions, coerced[lone], JSON.stringify(ownKeyUnits(coerced)));
// V8 runs toString twice for compound assignment while the spec and Kiln memoise once (ADR 0055).
coercions = 0;
coerced[coercingKey] += 1;
console.log("coercion-compound", coercions >= 1, coerced[lone], JSON.stringify(ownKeyUnits(coerced)));
