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
const define = new Function("Ctor", "Ctor.prototype.run = function(value) { return value; }; return Ctor.prototype.run(17);");
console.log(define(function Example() {}));
const append = new Function("this.res += '!'; return this.res;");
console.log(append.call({ res: "Hi" }));
