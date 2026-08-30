// `defineProperty` defines; it does not assign. The difference is the idiom where a prototype
// getter replaces itself with an own value the first time it is read.
const proto = {};
Object.defineProperty(proto, "lazy", {
  configurable: true,
  get() {
    Object.defineProperty(this, "lazy", { value: "computed", writable: true });
    return this.lazy;
  },
});
const instance = Object.create(proto);
console.log(instance.lazy, Object.getOwnPropertyDescriptor(instance, "lazy").value);

// An inherited setter takes an ordinary assignment, and does not take a definition.
let setterCalls = 0;
const withSetter = Object.create({
  set tracked(value) {
    setterCalls += 1;
  },
});
withSetter.tracked = 1;
Object.defineProperty(withSetter, "tracked", { value: 2 });
console.log(setterCalls, withSetter.tracked);

// A named function expression can call itself, and the name is visible nowhere else.
const fact = function step(n) {
  return n <= 1 ? 1 : n * step(n - 1);
};
console.log(fact(5), typeof step === "undefined");

// The name a function expression is *assigned* to is metadata, not a binding: reassigning the
// variable is visible from inside.
let handler = function () {
  return typeof handler;
};
const first = handler();
handler = 1;
console.log(first, typeof handler);

// `Array` and `Object` called without `new` do what constructing them does.
console.log(Array(3).length, JSON.stringify(Array(1, 2)));
console.log(typeof Object(), JSON.stringify(Object({ a: 1 })));
