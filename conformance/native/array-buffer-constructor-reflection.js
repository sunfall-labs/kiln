function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function checkConstructor(ctor, name, expectedKeys) {
  const keys = Reflect.ownKeys(ctor);
  const printable = keys.map(key => typeof key === "symbol" ? `@@${key.description}` : key);
  assert(JSON.stringify(printable) === JSON.stringify(expectedKeys), `${name} own keys: ${JSON.stringify(printable)}`);

  const length = Object.getOwnPropertyDescriptor(ctor, "length");
  assert(length.value === 1 && length.writable === false && length.enumerable === false && length.configurable === true,
    `${name}.length descriptor`);
  const constructorName = Object.getOwnPropertyDescriptor(ctor, "name");
  assert(constructorName.value === name && constructorName.writable === false && constructorName.enumerable === false && constructorName.configurable === true,
    `${name}.name descriptor`);
  const prototype = Object.getOwnPropertyDescriptor(ctor, "prototype");
  assert(prototype.value === ctor.prototype && prototype.writable === false && prototype.enumerable === false && prototype.configurable === false,
    `${name}.prototype descriptor`);
  const species = Object.getOwnPropertyDescriptor(ctor, Symbol.species);
  assert(typeof species.get === "function" && species.set === undefined && species.enumerable === false && species.configurable === true,
    `${name}[Symbol.species] descriptor`);
  assert(ctor[Symbol.species] === ctor, `${name}[Symbol.species] receiver`);
}

checkConstructor(ArrayBuffer, "ArrayBuffer", ["length", "name", "prototype", "isView", "@@species"]);
checkConstructor(SharedArrayBuffer, "SharedArrayBuffer", ["length", "name", "prototype", "@@species"]);
console.log("array-buffer constructor reflection ok");
