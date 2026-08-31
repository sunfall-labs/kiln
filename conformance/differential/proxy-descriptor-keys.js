// Descriptor reflection and own-key invariants share the proxy's internal methods.
function outcome(thunk) {
  try { return thunk(); } catch (error) { return error.name; }
}

const target = { value: 1 };
const seen = [];
const descriptorProxy = new Proxy(target, {
  getOwnPropertyDescriptor(t, key) {
    seen.push(String(key));
    return { value: 2, enumerable: true, configurable: true };
  },
});
const objectDescriptor = Object.getOwnPropertyDescriptor(descriptorProxy, "value");
const reflectDescriptor = Reflect.getOwnPropertyDescriptor(descriptorProxy, "value");
console.log(objectDescriptor.value, reflectDescriptor.value,
  Object.prototype.propertyIsEnumerable.call(descriptorProxy, "value"), seen.join(","));

const generic = new Proxy({ value: 1 }, {
  getOwnPropertyDescriptor() { return { enumerable: true, configurable: true }; },
});
const genericDescriptor = Object.getOwnPropertyDescriptor(generic, "value");
console.log("generic", "value" in genericDescriptor, genericDescriptor.enumerable,
  genericDescriptor.configurable);

// Omitted booleans are completed before the invariant check. A configurable target cannot be
// reported as a non-configurable property just because the trap left `configurable` out.
const incomplete = new Proxy({ value: 1 }, {
  getOwnPropertyDescriptor() { return { value: 1, enumerable: true }; },
});
console.log(
  outcome(() => Object.getOwnPropertyDescriptor(incomplete, "value")),
  outcome(() => Reflect.getOwnPropertyDescriptor(incomplete, "value")),
  outcome(() => Object.prototype.propertyIsEnumerable.call(incomplete, "value")),
);

function listed(label, target) {
  const proxy = new Proxy(target, { ownKeys: t => Reflect.ownKeys(t) });
  console.log(label, outcome(() => Reflect.ownKeys(proxy).map(String).join("|")));
}
listed("array", [1, 2]);
listed("function", function sample(a) {});
listed("boxed-string", new String("ab"));
listed("builtin", Array);

const inner = new Proxy({ nested: true }, { ownKeys: t => Reflect.ownKeys(t) });
listed("nested", inner);

// CreateListFromArrayLike accepts any object with a length and indexed properties.
const arrayLike = new Proxy({ answer: 1 }, {
  ownKeys() { return { 0: "answer", length: 1 }; },
});
console.log("array-like", outcome(() => Reflect.ownKeys(arrayLike).join("|")));
