for (const [name, constructor] of [
  ["Uint8Array", Uint8Array],
  ["Float16Array", Float16Array],
]) {
  const keys = Reflect.ownKeys(constructor);
  const symbols = Object.getOwnPropertySymbols(constructor);
  const ownSpecies = Object.getOwnPropertyDescriptor(constructor, Symbol.species);
  const inheritedSpecies = constructor[Symbol.species] === constructor;
  const abstractConstructor = Object.getPrototypeOf(constructor);
  const prototypeLink = Object.getPrototypeOf(constructor.prototype) === abstractConstructor.prototype;
  console.log(name, keys.join(","), symbols.length, ownSpecies === undefined, inheritedSpecies, prototypeLink);
  const customPrototype = Object.create(null);
  Object.setPrototypeOf(constructor, customPrototype);
  console.log(name, Object.getPrototypeOf(constructor) === customPrototype);
  Object.setPrototypeOf(constructor, abstractConstructor);
}

const abstractConstructor = Object.getPrototypeOf(Uint8Array);
const abstractKeys = Reflect.ownKeys(abstractConstructor);
const abstractSpecies = Object.getOwnPropertyDescriptor(abstractConstructor, Symbol.species);
console.log(
  abstractKeys.map((key) => typeof key === "symbol" ? `@@${key.description}` : key).join(","),
  abstractSpecies.get.name,
  abstractSpecies.get.length,
  abstractSpecies.enumerable,
  abstractSpecies.configurable,
);
