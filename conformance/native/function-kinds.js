function ordinary() {}
async function asynchronous() {}
function* generator() {}
async function* asyncGenerator() {}

const AsyncFunction = asynchronous.constructor;
const GeneratorFunction = generator.constructor;
const AsyncGeneratorFunction = asyncGenerator.constructor;
for (const constructor of [AsyncFunction, GeneratorFunction, AsyncGeneratorFunction]) {
  if (Reflect.ownKeys(constructor).join(",") !== "length,name,prototype") {
    throw new Error("function-kind constructors must expose the ordinary callable own keys");
  }
}
AsyncFunction.prototype.kindMarker = 11;

for (const prototype of [AsyncFunction.prototype, GeneratorFunction.prototype, AsyncGeneratorFunction.prototype]) {
  let rejected = false;
  try { prototype(); } catch (error) { rejected = error instanceof TypeError; }
  if (!rejected) throw new Error('function kind prototype objects must not be callable');
}
function freezeTarget() {}
Object.freeze(freezeTarget);
if (Object.getOwnPropertyDescriptor(freezeTarget, "prototype").writable) {
  throw new Error("freezing a function must make its implicit prototype slot read-only");
}

const generatorObjectPrototype = GeneratorFunction.prototype.prototype;
const asyncGeneratorObjectPrototype = AsyncGeneratorFunction.prototype.prototype;
const generatorInstance = generator();
const asyncGeneratorInstance = asyncGenerator();
if (
  generatorObjectPrototype === Object.prototype ||
  asyncGeneratorObjectPrototype === Object.prototype ||
  Reflect.ownKeys(generatorObjectPrototype).length !== 5 ||
  Reflect.ownKeys(asyncGeneratorObjectPrototype).length !== 5 ||
  Object.getPrototypeOf(generatorObjectPrototype) !== Iterator.prototype ||
  typeof generatorObjectPrototype.next !== "function" ||
  typeof generatorObjectPrototype.return !== "function" ||
  typeof generatorObjectPrototype.throw !== "function" ||
  generatorObjectPrototype.constructor !== GeneratorFunction.prototype ||
  Object.getOwnPropertyDescriptor(generatorObjectPrototype, "constructor").writable ||
  Object.prototype.toString.call(generatorObjectPrototype) !== "[object Generator]" ||
  Object.getPrototypeOf(asyncGeneratorObjectPrototype) === Object.prototype ||
  typeof Object.getPrototypeOf(asyncGeneratorObjectPrototype)[Symbol.asyncIterator] !== "function" ||
  typeof asyncGeneratorObjectPrototype.next !== "function" ||
  typeof asyncGeneratorObjectPrototype.return !== "function" ||
  typeof asyncGeneratorObjectPrototype.throw !== "function" ||
  asyncGeneratorObjectPrototype.constructor !== AsyncGeneratorFunction.prototype ||
  Object.getOwnPropertyDescriptor(asyncGeneratorObjectPrototype, "constructor").writable ||
  Object.prototype.toString.call(asyncGeneratorObjectPrototype) !== "[object AsyncGenerator]" ||
  Object.getOwnPropertyDescriptor(GeneratorFunction.prototype, "prototype").writable ||
  Object.getOwnPropertyDescriptor(AsyncGeneratorFunction.prototype, "prototype").writable ||
  Object.getPrototypeOf(generator.prototype) !== generatorObjectPrototype ||
  Object.getPrototypeOf(asyncGenerator.prototype) !== asyncGeneratorObjectPrototype ||
  Object.getPrototypeOf(generatorInstance) !== generator.prototype ||
  Object.getPrototypeOf(asyncGeneratorInstance) !== asyncGenerator.prototype
) {
  throw new Error("native generator function prototypes do not match their shared intrinsics");
}

console.log(
  AsyncFunction !== Function,
  GeneratorFunction !== Function,
  AsyncGeneratorFunction !== Function,
  AsyncFunction !== GeneratorFunction,
  GeneratorFunction !== AsyncGeneratorFunction,
);
console.log(
  typeof AsyncFunction.prototype,
  typeof GeneratorFunction.prototype,
  typeof AsyncGeneratorFunction.prototype,
  Object.getPrototypeOf(AsyncFunction) === Function,
  AsyncFunction.length,
  Object.prototype.toString.call(asynchronous),
  Object.prototype.toString.call(generator),
  Object.prototype.toString.call(asyncGenerator),
);
console.log(
  asynchronous instanceof AsyncFunction,
  asynchronous instanceof GeneratorFunction,
  generator instanceof GeneratorFunction,
  asyncGenerator instanceof AsyncGeneratorFunction,
  ordinary instanceof AsyncFunction,
);
console.log(
  Object.getPrototypeOf(asynchronous) === AsyncFunction.prototype,
  Object.getPrototypeOf(generator) === GeneratorFunction.prototype,
  Object.getPrototypeOf(asyncGenerator) === AsyncGeneratorFunction.prototype,
  Object.getPrototypeOf(ordinary) === Function.prototype,
  asynchronous.kindMarker === 11,
);
Object.setPrototypeOf(asynchronous, { customMarker: 12 });
console.log(asynchronous.kindMarker === undefined, asynchronous.customMarker === 12);

asynchronous.constructor = ordinary;
console.log(asynchronous.constructor === ordinary);
if (!Reflect.deleteProperty(asynchronous, "constructor") ||
    Reflect.ownKeys(asynchronous).includes("constructor")) {
  throw new Error("deleting a configurable closure property must remove the own property");
}
Object.defineProperty(asynchronous, "lockedKindTag", { value: 1, configurable: false });
if (Reflect.deleteProperty(asynchronous, "lockedKindTag") || asynchronous.lockedKindTag !== 1) {
  throw new Error("deleting a non-configurable closure property must fail");
}
Object.setPrototypeOf(asynchronous, Function.prototype);
console.log(asynchronous.constructor === Function, asynchronous instanceof AsyncFunction);
Object.setPrototypeOf(asynchronous, null);
console.log(asynchronous.constructor === undefined, asynchronous.toString === undefined);
