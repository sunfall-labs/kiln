const map = new Map([["a", 1], ["b", 2]]);
const mapSize = Object.getOwnPropertyDescriptor(Map.prototype, "size");
console.log(mapSize.get.name, mapSize.get.length, mapSize.set, mapSize.enumerable,
  mapSize.configurable);
console.log(mapSize.get.call(map), map.size, Object.hasOwn(map, "size"));

const set = new Set([1, 2, 3]);
const setSize = Object.getOwnPropertyDescriptor(Set.prototype, "size");
console.log(setSize.get.name, setSize.get.length, setSize.set, setSize.enumerable,
  setSize.configurable);
console.log(setSize.get.call(set), set.size, Object.hasOwn(set, "size"));

for (const getter of [mapSize.get, setSize.get]) {
  try { getter.call({}); } catch (error) { console.log(error instanceof TypeError); }
}
