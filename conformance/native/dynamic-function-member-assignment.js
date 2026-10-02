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
keyCalls = keyCoercions = getterCalls = setterCalls = 0;
const updateOnce = new Function("object", "key", "return object[key()]++;");
if (updateOnce(object, dynamicKey) !== 9 || keyCalls !== 1 || keyCoercions !== 2 ||
    getterCalls !== 1 || setterCalls !== 1 || value !== 10) {
  throw new Error("computed member update effects must run once");
}
const define = new Function("Ctor", "Ctor.prototype.run = function(value) { return value; }; return Ctor.prototype.run(17);");
console.log(define(function Example() {}));
const append = new Function("this.res += '!'; return this.res;");
console.log(append.call({ res: "Hi" }));
const count = new Function("return ++this.now;");
console.log(count.call({ now: 4 }));
