// Every `Object` static and every `Object.prototype` method starts with `ToObject(this)`, and
// both engines skipped that step in the same places. Only what the native backend can already
// express is here: it has no wrapper object to box a primitive into, so `Object.assign(1, …)` and
// `Object.prototype.valueOf.call(true)` are refusals there rather than lines to compare, and
// `Boolean.prototype` is not a value it has at all.

// A primitive is asked about the object it stands for, so its chain is the wrapper prototype.
console.log("string", Object.getPrototypeOf("abc") === String.prototype);
console.log("number", Object.getPrototypeOf(0) === Number.prototype);

// `Math` and `JSON` are ordinary objects, not functions. Reading `Function.prototype` off them
// made `Math instanceof Function` true and gave a value with no call behaviour a `call`.
console.log("namespaces", Object.getPrototypeOf(Math) === Object.prototype, Object.getPrototypeOf(JSON) === Object.prototype);

// A built-in's members are writable and configurable, so it is neither frozen nor sealed. A
// primitive has nothing to freeze and is frozen by definition.
console.log("integrity", Object.isFrozen(Object), Object.isFrozen(Math), Object.isFrozen(1));

// `toLocaleString` calls the receiver's own `toString` rather than producing the tag itself.
const shape = { toString() { return "mine"; } };
console.log("locale", shape.toLocaleString());
console.log("locale-inherited", Object.create(shape).toLocaleString());
