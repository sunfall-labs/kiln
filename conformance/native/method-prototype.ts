// Methods (object methods, class methods, static methods, accessors) do NOT
// carry an own `.prototype` property (unless they are generators), and calling
// `new` on any method throws `TypeError: a function is not a constructor`.
// Ordinary functions and class constructors retain `.prototype` and can be constructed.

// 1. Ordinary function: has own prototype and is constructable
function Ordinary() {}
console.log("ordinary hasOwn:", Object.hasOwn(Ordinary, "prototype"));
console.log("ordinary typeof proto:", typeof Ordinary.prototype);
const ordInstance = new (Ordinary as any)();
console.log("ordinary constructed:", ordInstance instanceof Ordinary);

// 2. Object literal method: no prototype, not constructable
const obj = {
  m() { return 42; },
  *gen() { yield 1; },
  get prop() { return 10; },
};

console.log("obj.m hasOwn:", Object.hasOwn(obj.m, "prototype"));
console.log("obj.m in proto:", "prototype" in obj.m);
console.log("obj.m typeof proto:", typeof (obj.m as any).prototype);

try {
  new (obj.m as any)();
  console.log("obj.m new succeeded unexpectedly");
} catch (e: any) {
  console.log("obj.m new caught:", e.name, e.message);
}

// 3. Class method and static method: no prototype, not constructable
class Example {
  instanceMethod() { return 1; }
  static staticMethod() { return 2; }
}

console.log("instanceMethod hasOwn:", Object.hasOwn(Example.prototype.instanceMethod, "prototype"));
console.log("instanceMethod proto:", (Example.prototype.instanceMethod as any).prototype);
try {
  new (Example.prototype.instanceMethod as any)();
  console.log("instanceMethod new succeeded unexpectedly");
} catch (e: any) {
  console.log("instanceMethod new caught:", e.name, e.message);
}

console.log("staticMethod hasOwn:", Object.hasOwn(Example.staticMethod, "prototype"));
console.log("staticMethod proto:", (Example.staticMethod as any).prototype);
try {
  new (Example.staticMethod as any)();
  console.log("staticMethod new succeeded unexpectedly");
} catch (e: any) {
  console.log("staticMethod new caught:", e.name, e.message);
}

// 4. Class constructor itself retains prototype and is constructable
console.log("class constructor hasOwn:", Object.hasOwn(Example, "prototype"));
console.log("class constructor typeof proto:", typeof Example.prototype);
const exInstance = new Example();
console.log("class constructed:", exInstance instanceof Example);

// 5. Generator method: has prototype (for iterator instances), but not constructable
console.log("generator method hasOwn:", Object.hasOwn(obj.gen, "prototype"));
console.log("generator method typeof proto:", typeof (obj.gen as any).prototype);
try {
  new (obj.gen as any)();
  console.log("generator method new succeeded unexpectedly");
} catch (e: any) {
  console.log("generator method new caught:", e.name, e.message);
}

// 6. Accessor functions: no prototype, not constructable
const desc = Object.getOwnPropertyDescriptor(obj, "prop")!;
console.log("getter hasOwn:", Object.hasOwn(desc.get!, "prototype"));
console.log("getter proto:", (desc.get as any).prototype);
try {
  new (desc.get as any)();
  console.log("getter new succeeded unexpectedly");
} catch (e: any) {
  console.log("getter new caught:", e.name, e.message);
}
