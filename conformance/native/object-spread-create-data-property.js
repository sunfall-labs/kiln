// CopyDataProperties defines each copied key as a fresh own data property. It must not use
// [[Set]] and trigger an inherited setter on the target's prototype.
const stringKey = "__kiln_copy_data_property_string__";
const symbolKey = Symbol("kiln copy data property");
let inheritedSetterCalls = 0;
Object.defineProperty(Object.prototype, stringKey, {
  configurable: true,
  set() { inheritedSetterCalls += 1; },
});
Object.defineProperty(Object.prototype, symbolKey, {
  configurable: true,
  set() { inheritedSetterCalls += 1; },
});

function defineNamed(target, label) {
  Object.defineProperty(target, stringKey, {
    configurable: true,
    enumerable: true,
    value: `${label}-string`,
    writable: true,
  });
  Object.defineProperty(target, symbolKey, {
    configurable: true,
    enumerable: true,
    value: `${label}-symbol`,
    writable: true,
  });
}

const ordinary = {};
defineNamed(ordinary, "object");
// Force the ordinary source through its dictionary/descriptor path.
Object.defineProperty(ordinary, "hidden", { value: 1, enumerable: false });

const array = [];
defineNamed(array, "array");

const proxyTarget = {};
defineNamed(proxyTarget, "proxy");
const proxy = new Proxy(proxyTarget, {
  ownKeys(target) { return Reflect.ownKeys(target); },
  getOwnPropertyDescriptor(target, key) {
    return Reflect.getOwnPropertyDescriptor(target, key);
  },
  get(target, key, receiver) { return Reflect.get(target, key, receiver); },
});

for (const [label, source] of [["object", ordinary], ["array", array], ["proxy", proxy]]) {
  const copy = { ...source };
  if (copy[stringKey] !== `${label}-string` || copy[symbolKey] !== `${label}-symbol` ||
      !Object.prototype.hasOwnProperty.call(copy, stringKey) ||
      !Object.prototype.hasOwnProperty.call(copy, symbolKey)) {
    throw new Error(`${label} spread did not define own string and symbol properties`);
  }
}
Object.defineProperty(Object.prototype, "0", {
  configurable: true,
  set() { inheritedSetterCalls += 1; },
});
const indexedCopy = { ...["array-index"] };
delete Object.prototype["0"];
if (indexedCopy[0] !== "array-index" ||
    !Object.prototype.hasOwnProperty.call(indexedCopy, "0")) {
  throw new Error("array spread did not define its indexed property");
}
if (inheritedSetterCalls !== 0) {
  throw new Error(`spread invoked ${inheritedSetterCalls} inherited setter(s)`);
}

const mutationSymbol = Symbol("deleted during spread");
const mutationAddedSymbol = Symbol("added during spread");

function mutationSource(arraySource = false) {
  const source = arraySource ? ["index"] : {};
  Object.defineProperty(source, "first", {
    configurable: true,
    enumerable: true,
    get() {
      delete source.later;
      delete source[mutationSymbol];
      source.added = "too late";
      source[mutationAddedSymbol] = "too late too";
      source.second = "updated";
      return "first value";
    },
  });
  Object.defineProperty(source, "later", {
    configurable: true,
    enumerable: true,
    value: "deleted before its turn",
    writable: true,
  });
  Object.defineProperty(source, "second", {
    configurable: true,
    enumerable: true,
    value: "original",
    writable: true,
  });
  Object.defineProperty(source, mutationSymbol, {
    configurable: true,
    enumerable: true,
    value: "symbol deleted before its turn",
    writable: true,
  });
  return source;
}

function checkMutation(label, source) {
  const copy = { ...source };
  if (copy.first !== "first value" || copy.second !== "updated" ||
      Object.prototype.hasOwnProperty.call(copy, "later") ||
      Object.prototype.hasOwnProperty.call(copy, "added") ||
      Object.prototype.hasOwnProperty.call(copy, mutationSymbol) ||
      Object.prototype.hasOwnProperty.call(copy, mutationAddedSymbol)) {
    throw new Error(`${label} spread did not use the original key snapshot and fresh descriptors`);
  }
}

checkMutation("object", mutationSource());
checkMutation("array", mutationSource(true));
const typedSymbol = Symbol("typed array enumerable key");
const typedLateSymbol = Symbol("typed array added during spread");
const typedSource = new Uint8Array([41, 42, 43]);
Object.defineProperty(typedSource, "first", {
  configurable: true,
  enumerable: true,
  get() {
    delete typedSource.later;
    typedSource.added = "too late";
    typedSource.second = "updated";
    typedSource[typedLateSymbol] = "too late too";
    return "first value";
  },
});
Object.defineProperty(typedSource, "later", {
  configurable: true,
  enumerable: true,
  value: "deleted before its turn",
  writable: true,
});
Object.defineProperty(typedSource, "second", {
  configurable: true,
  enumerable: true,
  value: "original",
  writable: true,
});
Object.defineProperty(typedSource, typedSymbol, {
  configurable: true,
  enumerable: true,
  value: "typed symbol value",
});
const typedCopy = { ...typedSource };
if (typedCopy[0] !== 41 || typedCopy[1] !== 42 || typedCopy[2] !== 43 ||
    typedCopy.first !== "first value" || typedCopy.second !== "updated" ||
    typedCopy[typedSymbol] !== "typed symbol value" ||
    Object.prototype.hasOwnProperty.call(typedCopy, "later") ||
    Object.prototype.hasOwnProperty.call(typedCopy, "added") ||
    Object.prototype.hasOwnProperty.call(typedCopy, typedLateSymbol)) {
  throw new Error("typed array spread dropped named or symbol keys or missed fresh descriptors");
}
const proxyMutationTarget = mutationSource();
checkMutation("proxy", new Proxy(proxyMutationTarget, {
  ownKeys(target) { return Reflect.ownKeys(target); },
  getOwnPropertyDescriptor(target, key) {
    return Reflect.getOwnPropertyDescriptor(target, key);
  },
  get(target, key, receiver) { return Reflect.get(target, key, receiver); },
}));

delete Object.prototype[stringKey];
delete Object.prototype[symbolKey];
console.log("CopyDataProperties created own string and symbol data properties");
