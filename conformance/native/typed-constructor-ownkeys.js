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
}
