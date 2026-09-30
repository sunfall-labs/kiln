function ordinary() {}
async function asynchronous() {}
function* generator() {}
async function* asyncGenerator() {}

const AsyncFunction = asynchronous.constructor;
const GeneratorFunction = generator.constructor;
const AsyncGeneratorFunction = asyncGenerator.constructor;
AsyncFunction.prototype.kindMarker = 11;

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
  Object.getPrototypeOf(AsyncFunction) === Function.prototype,
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
Object.setPrototypeOf(asynchronous, { customMarker: 12 });
console.log(asynchronous.kindMarker === undefined, asynchronous.customMarker === 12);
console.log(
  Object.getPrototypeOf(asynchronous) === AsyncFunction.prototype,
  Object.getPrototypeOf(generator) === GeneratorFunction.prototype,
  Object.getPrototypeOf(asyncGenerator) === AsyncGeneratorFunction.prototype,
  Object.getPrototypeOf(ordinary) === Function.prototype,
  asynchronous.kindMarker === 11,
);

asynchronous.constructor = ordinary;
console.log(asynchronous.constructor === ordinary);
delete asynchronous.constructor;
Object.setPrototypeOf(asynchronous, Function.prototype);
console.log(asynchronous.constructor === Function, asynchronous instanceof AsyncFunction);
