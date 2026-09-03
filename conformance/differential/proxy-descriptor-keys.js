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

// V8 gives a *non-strict* ordinary function own `arguments` and `caller` data properties. The
// specification permits that and does not require it: §17 forbids the pair only on strict
// functions, classes, generators, async functions and built-ins, which leaves sloppy functions as
// the one place a legacy extension may still live. kiln does not implement it -- it keeps the
// restricted accessors on `Function.prototype`, so `sample.caller` throws a TypeError here where
// node answers `null`.
//
// That difference is loud at the point it happens, which is the property this corpus exists to
// protect, so the two names are dropped from the comparison rather than pinning kiln to an
// extension it has chosen not to grow. Everything else about a proxy's `ownKeys` over a function
// target -- that the trap runs, that the result is a list of strings, that the order is the
// target's -- is still compared byte for byte.
const legacyFunctionKeys = new Set(["arguments", "caller"]);
function listed(label, target) {
  const proxy = new Proxy(target, { ownKeys: t => Reflect.ownKeys(t) });
  console.log(label, outcome(() => Reflect.ownKeys(proxy)
    .map(String)
    .filter(key => !legacyFunctionKeys.has(key))
    .join("|")));
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
