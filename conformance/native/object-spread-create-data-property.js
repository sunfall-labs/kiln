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

function mutationSource(arraySource = false) {
  const source = arraySource ? ["index"] : {};
  Object.defineProperty(source, "first", {
    configurable: true,
    enumerable: true,
    get() {
      delete source.later;
      source.added = "too late";
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
  return source;
}

function checkMutation(label, source) {
  const copy = { ...source };
  if (copy.first !== "first value" || copy.second !== "updated" ||
      Object.prototype.hasOwnProperty.call(copy, "later") ||
      Object.prototype.hasOwnProperty.call(copy, "added")) {
    throw new Error(`${label} spread did not use the original key snapshot and fresh descriptors`);
  }
}

checkMutation("object", mutationSource());
checkMutation("array", mutationSource(true));
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
