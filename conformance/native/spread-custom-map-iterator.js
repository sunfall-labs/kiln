// kiln-native: profile=native-release
// Array spread on a collection must use its @@iterator property, including an override on a
// Map subclass. The backing Map is intentionally empty so a collection-internal shortcut is
// observably different from the protocol.
class CustomMap extends Map {
  *[Symbol.iterator]() {
    yield ["custom", this.size];
  }
}

const custom = new CustomMap();
custom.set("stored", 1);
console.log(JSON.stringify([...custom]));
console.log(JSON.stringify([...new Map([["stored", 1]])]));

const original = Map.prototype[Symbol.iterator];
Map.prototype[Symbol.iterator] = function* () {
  yield ["overridden", this.size];
};
console.log(JSON.stringify([...new Map([["stored", 1]])]));
Map.prototype[Symbol.iterator] = original;
