// Methods (object methods, class methods, static methods, accessors) do not have an
// own `.prototype` property (unless they are generators), and calling `new` on them throws TypeError.

function Ordinary() {}
console.log(Ordinary.hasOwnProperty("prototype"), typeof Ordinary.prototype);
console.log(new Ordinary() instanceof Ordinary);

const obj = {
  m() { return 42; },
  *gen() { yield 1; },
  get prop() { return 10; },
};

console.log(obj.m.hasOwnProperty("prototype"), "prototype" in obj.m, obj.m.prototype);
try {
  new obj.m();
  console.log("unexpected success");
} catch (e) {
  console.log(e.name);
}

class Example {
  instanceMethod() { return 1; }
  static staticMethod() { return 2; }
}

console.log(Example.prototype.instanceMethod.hasOwnProperty("prototype"), Example.prototype.instanceMethod.prototype);
try {
  new Example.prototype.instanceMethod();
} catch (e) {
  console.log(e.name);
}

console.log(Example.staticMethod.hasOwnProperty("prototype"), Example.staticMethod.prototype);
try {
  new Example.staticMethod();
} catch (e) {
  console.log(e.name);
}

console.log(Example.hasOwnProperty("prototype"), typeof Example.prototype);
console.log(new Example() instanceof Example);

console.log(obj.gen.hasOwnProperty("prototype"), typeof obj.gen.prototype);
try {
  new obj.gen();
} catch (e) {
  console.log(e.name);
}

const desc = Object.getOwnPropertyDescriptor(obj, "prop");
console.log(desc.get.hasOwnProperty("prototype"), desc.get.prototype);
try {
  new desc.get();
} catch (e) {
  console.log(e.name);
}
