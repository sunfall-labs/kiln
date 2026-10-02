const order = [];
let stored = 2;
const inner = {};
Object.defineProperty(inner, "count", {
  get() { order.push("get"); return stored; },
  set(value) { order.push("set"); stored = value; },
  configurable: true,
});
const update = new Function("obj", "key", "rhs", "obj.inner[key()] += rhs(); return obj.inner.count;");
console.log(update({ inner }, () => { order.push("key"); return "count"; }, () => { order.push("rhs"); return 3; }));
console.log(order.join(","));

// Assignment recognition must not evaluate a computed member while looking for `=`. These
// Function-created bodies exercise the dynamic parser's ordinary read, assignment, and update
// paths, including ToPropertyKey and accessor effects.
let keyCalls = 0;
let keyCoercions = 0;
let getterCalls = 0;
let setterCalls = 0;
let value = 4;
const keyObject = {
  [Symbol.toPrimitive]() { keyCoercions++; return "value"; },
};
const object = {};
Object.defineProperty(object, "value", {
  configurable: true,
  get() { getterCalls++; return value; },
  set(next) { setterCalls++; value = next; },
});
const dynamicKey = () => { keyCalls++; return keyObject; };
const read = new Function("object", "key", "return object[key()];");
if (read(object, dynamicKey) !== 4 || keyCalls !== 1 || keyCoercions !== 1 ||
    getterCalls !== 1 || setterCalls !== 0) {
  throw new Error("computed member read effects must run once");
}
keyCalls = keyCoercions = getterCalls = setterCalls = 0;
const assign = new Function("object", "key", "rhs", "return object[key()] = rhs();");
if (assign(object, dynamicKey, () => 9) !== 9 || keyCalls !== 1 || keyCoercions !== 1 ||
    getterCalls !== 0 || setterCalls !== 1 || value !== 9) {
  throw new Error("computed member assignment effects must run once");
}
const assignmentOrder = [];
const assignAfterRhs = new Function("object", "key", "rhs",
  "object[key()] = rhs(); return 0;");
assignAfterRhs(object,
  () => {
    assignmentOrder.push("key");
    return { [Symbol.toPrimitive]() { assignmentOrder.push("coerce"); return "value"; } };
  },
  () => {
    const scratch = [{ marker: "rhs" }];
    assignmentOrder.push(scratch[0].marker);
    return 11;
  });
if (assignmentOrder.join(",") !== "key,rhs,coerce") {
  throw new Error("simple assignment must coerce its final key after the RHS");
}
value = 9;
keyCalls = keyCoercions = getterCalls = setterCalls = 0;
const updateOnce = new Function("object", "key", "return object[key()]++;");
if (updateOnce(object, dynamicKey) !== 9 || keyCalls !== 1 || keyCoercions !== 2 ||
    getterCalls !== 1 || setterCalls !== 1 || value !== 10) {
  throw new Error("computed member update effects must run once");
}
const updateOrder = [];
const orderedUpdateKey = {
  [Symbol.toPrimitive]() { updateOrder.push("coerce"); return "value"; },
};
const orderedUpdateObject = {};
Object.defineProperty(orderedUpdateObject, "value", {
  configurable: true,
  get() { updateOrder.push("get"); return 1; },
  set() { updateOrder.push("set"); },
});
const orderedUpdate = new Function("object", "key", "return object[key()]++;");
if (orderedUpdate(orderedUpdateObject, () => {
  updateOrder.push("key");
  return orderedUpdateKey;
}) !== 1 || updateOrder.join(",") !== "key,coerce,get,coerce,set") {
  throw new Error("computed update must retain the raw key for both GetValue and PutValue");
}
const compoundOrder = [];
const compoundKey = {
  [Symbol.toPrimitive]() { compoundOrder.push("coerce"); return "value"; },
};
const compoundObject = {};
Object.defineProperty(compoundObject, "value", {
  configurable: true,
  get() { compoundOrder.push("get"); return 1; },
  set() { compoundOrder.push("set"); },
});
new Function("object", "key", "rhs", "object[key()] += rhs();")(
  compoundObject,
  () => { compoundOrder.push("key"); return compoundKey; },
  () => { compoundOrder.push("rhs"); return 2; },
);
if (compoundOrder.join(",") !== "key,coerce,get,rhs,coerce,set") {
  throw new Error("compound assignment must coerce at GetValue and PutValue");
}
const logicalOrder = [];
const logicalKey = {
  [Symbol.toPrimitive]() { logicalOrder.push("coerce"); return "value"; },
};
const logicalObject = {};
Object.defineProperty(logicalObject, "value", {
  configurable: true,
  get() { logicalOrder.push("get"); return 0; },
  set() { logicalOrder.push("set"); },
});
new Function("object", "key", "rhs", "object[key()] ||= rhs();")(
  logicalObject,
  () => { logicalOrder.push("key"); return logicalKey; },
  () => { logicalOrder.push("rhs"); return 2; },
);
if (logicalOrder.join(",") !== "key,coerce,get,rhs,coerce,set") {
  throw new Error("logical assignment must coerce at GetValue and PutValue");
}
const define = new Function("Ctor", "Ctor.prototype.run = function(value) { return value; }; return Ctor.prototype.run(17);");
console.log(define(function Example() {}));
const append = new Function("this.res += '!'; return this.res;");
console.log(append.call({ res: "Hi" }));
const count = new Function("return ++this.now;");
console.log(count.call({ now: 4 }));
