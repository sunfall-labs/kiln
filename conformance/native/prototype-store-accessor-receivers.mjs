function check(condition, message) {
  if (!condition) throw new Error(message);
}

const key = "__kilnReceiverMatrixProbe__";
const objectOnlyKey = "__kilnObjectPrototypeReceiverProbe__";
const proxyKey = "__kilnProxyParentReceiverProbe__";
const setProxyKey = "__kilnSetParentReceiverProbe__";
const bigintProxyKey = "__kilnBigIntParentReceiverProbe__";
const installed = [];
const object = { label: "object receiver" };
const array = [1, 2];
const regexp = /receiver/;
const map = new Map();
const set = new Set();
const promise = Promise.resolve("ready");
function callable() {}
const symbol = Symbol("receiver");
const explicitReceiver = { label: "explicit receiver" };
const inheritedFromBuiltin = Object.create(Array.prototype);
const proxyReceiverCounts = { get: 0, getPrototypeOf: 0, has: 0 };
const proxyParent = new Proxy({}, {
  get(_target, property, receiver) {
    if (property === proxyKey) {
      proxyReceiverCounts.get += 1;
      return receiver;
    }
    return undefined;
  },
  getPrototypeOf() {
    proxyReceiverCounts.getPrototypeOf += 1;
    return null;
  },
  has() {
    proxyReceiverCounts.has += 1;
    return false;
  },
});
const inheritedFromProxy = Object.create(proxyParent);
const originalArrayPrototypeParent = Reflect.getPrototypeOf(Array.prototype);
const originalSetParent = Reflect.getPrototypeOf(Set);
const originalBigIntParent = Reflect.getPrototypeOf(BigInt);
const originalUint8ArrayParent = Reflect.getPrototypeOf(Uint8Array);
const originalObjectMap = Object.getOwnPropertyDescriptor(Object.prototype, "map");
const staticProxyCounts = { get: 0, has: 0 };
const staticProxyParent = new Proxy({}, {
  get(_target, property, receiver) {
    staticProxyCounts.get += 1;
    if (property === setProxyKey || property === bigintProxyKey || property === "from") {
      return receiver;
    }
    return undefined;
  },
  has() {
    staticProxyCounts.has += 1;
    return false;
  },
});

function install(prototype) {
  const old = Object.getOwnPropertyDescriptor(prototype, key);
  installed.push([prototype, old]);
  Object.defineProperty(prototype, key, {
    configurable: true,
    get() { return this; },
  });
}

try {
  Object.defineProperty(Object.prototype, objectOnlyKey, {
    configurable: true,
    get() { return this; },
  });
  check(Reflect.get(Array.prototype, objectOnlyKey, explicitReceiver) === explicitReceiver,
    "an unmodified builtin prototype uses its implicit parent with the explicit receiver");

  for (const prototype of [
    Object.prototype,
    Array.prototype,
    RegExp.prototype,
    String.prototype,
    Number.prototype,
    Boolean.prototype,
    Symbol.prototype,
    BigInt.prototype,
    Map.prototype,
    Set.prototype,
    Promise.prototype,
    Function.prototype,
  ]) install(prototype);

  check(object[key] === object, "Object.prototype getter receives the original object");
  check(array[key] === array, "Array.prototype getter receives the original array");
  check(regexp[key] === regexp, "RegExp.prototype getter receives the original RegExp");
  check("string receiver"[key] === "string receiver",
    "String.prototype getter receives the strict primitive receiver");
  check((37)[key] === 37, "Number.prototype getter receives the strict primitive receiver");
  check(true[key] === true, "Boolean.prototype getter receives the strict primitive receiver");
  check(symbol[key] === symbol, "Symbol.prototype getter receives the strict primitive receiver");
  check((41n)[key] === 41n, "BigInt.prototype getter receives the strict primitive receiver");
  check(map[key] === map, "Map.prototype getter receives the original Map");
  check(set[key] === set, "Set.prototype getter receives the original Set");
  check(promise[key] === promise, "Promise.prototype getter receives the original Promise");
  check(callable[key] === callable, "Function.prototype getter receives the original function");
  check(Reflect.get(Object.prototype, key, explicitReceiver) === explicitReceiver,
    "Reflect.get preserves its explicit receiver on Object.prototype");
  check(Reflect.get(Array.prototype, key, explicitReceiver) === explicitReceiver,
    "Reflect.get preserves its explicit receiver when reading a builtin prototype store");
  let inheritedMapGetterCalls = 0;
  Object.defineProperty(Object.prototype, "map", {
    configurable: true,
    get() {
      inheritedMapGetterCalls += 1;
      return "shadowed by Array.prototype.map";
    },
  });
  check(typeof Reflect.get(Array.prototype, "map", explicitReceiver) === "function" &&
        inheritedMapGetterCalls === 0,
    "an intrinsic own method shadows an inherited accessor during Reflect.get");
  if (originalObjectMap) Object.defineProperty(Object.prototype, "map", originalObjectMap);
  else delete Object.prototype.map;
  check(inheritedFromBuiltin[key] === inheritedFromBuiltin,
    "an object inheriting from a builtin prototype preserves its receiver");

  check("primitive receiver"[objectOnlyKey] === "primitive receiver",
    "Object.prototype fallback preserves a primitive receiver");
  check(Reflect.get(Array.prototype, objectOnlyKey, explicitReceiver) === explicitReceiver,
    "builtin prototype fallback preserves the explicit receiver");
  check(Reflect.get([10], "0", explicitReceiver) === 10,
    "receiver-aware reads preserve dense array index values");
  const indexedAccessor = [];
  Object.defineProperty(indexedAccessor, "0", { configurable: true, get() { return this; } });
  check(Reflect.get(indexedAccessor, "0", explicitReceiver) === explicitReceiver,
    "an array index accessor receives Reflect.get's explicit receiver");
  const indexedProxy = new Proxy(indexedAccessor, {});
  check(Reflect.get(indexedProxy, "0", explicitReceiver) === explicitReceiver,
    "proxy forwarding preserves the explicit receiver for array index accessors");
  const symbolReadArray = [];
  Object.defineProperty(symbolReadArray, Symbol.iterator, {
    configurable: true,
    get() { return this; },
  });
  check(Reflect.get(symbolReadArray, Symbol.iterator, explicitReceiver) === explicitReceiver,
    "an array symbol-keyed own accessor receives Reflect.get's explicit receiver");
  check(symbolReadArray[Symbol.iterator] === symbolReadArray,
    "an array symbol-keyed own accessor receives the array as receiver");
  for (const constructor of [Array, Set, Uint8Array]) {
    check(Reflect.get(constructor, Symbol.species, explicitReceiver) === explicitReceiver,
      `${constructor.name} species uses the explicit receiver`);
  }
  class DerivedArray extends Array {}
  check(Reflect.get(Array, Symbol.species, DerivedArray) === DerivedArray,
    "an inherited species getter returns the derived constructor receiver");
  check(Reflect.get(inheritedFromProxy, proxyKey, explicitReceiver) === explicitReceiver,
    "a proxy parent receives the original Reflect.get receiver");
  check(inheritedFromProxy[proxyKey] === inheritedFromProxy,
    "ordinary reads also reach the proxy parent get trap for an uncached key");
  check(proxyReceiverCounts.get === 2 && proxyReceiverCounts.getPrototypeOf === 0 &&
      proxyReceiverCounts.has === 0,
    "proxy parent lookup invokes get without spurious has or getPrototypeOf traps");

  Object.setPrototypeOf(Set, staticProxyParent);
  Object.setPrototypeOf(BigInt, staticProxyParent);
  Object.setPrototypeOf(Uint8Array, staticProxyParent);
  check(Reflect.get(Set, setProxyKey, explicitReceiver) === explicitReceiver,
    "Set replacement-parent Get preserves the explicit receiver");
  check(Reflect.get(BigInt, bigintProxyKey, explicitReceiver) === explicitReceiver,
    "BigInt replacement-parent Get preserves the explicit receiver");
  check(Reflect.get(Uint8Array, "from", explicitReceiver) === explicitReceiver,
    "typed-array inherited static Get preserves the explicit receiver");
  check(staticProxyCounts.get === 3 && staticProxyCounts.has === 0,
    "Set, BigInt, and typed-array replacement parents use Get without HasProperty");

  const relinkedArrayParent = Object.create(proxyParent);
  Object.setPrototypeOf(Array.prototype, relinkedArrayParent);
  proxyReceiverCounts.get = 0;
  proxyReceiverCounts.getPrototypeOf = 0;
  check(Reflect.get(Array.prototype, proxyKey, explicitReceiver) === explicitReceiver,
    "a relinked builtin proxy parent receives the explicit receiver");
  check(proxyReceiverCounts.get === 1 && proxyReceiverCounts.getPrototypeOf === 0 &&
      proxyReceiverCounts.has === 0,
    "relinked builtin proxy parent gets one Get with no has or duplicate prototype trap");

  console.log("prototype store accessor receivers pass");
} finally {
  Object.setPrototypeOf(Array.prototype, originalArrayPrototypeParent);
  Object.setPrototypeOf(Set, originalSetParent);
  Object.setPrototypeOf(BigInt, originalBigIntParent);
  Object.setPrototypeOf(Uint8Array, originalUint8ArrayParent);
  delete Object.prototype[objectOnlyKey];
  if (originalObjectMap) Object.defineProperty(Object.prototype, "map", originalObjectMap);
  else delete Object.prototype.map;
  for (const [prototype, descriptor] of installed.reverse()) {
    if (descriptor) Object.defineProperty(prototype, key, descriptor);
    else delete prototype[key];
  }
}
