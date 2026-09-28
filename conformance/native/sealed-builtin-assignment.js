// Existing data properties of sealed built-in stores can be assigned without changing their
// descriptors. The same source also runs under native-realms.
const dateProto = Date.prototype;
const dateMethod = dateProto.toISOString;
const dateDescriptor = Object.getOwnPropertyDescriptor(dateProto, "toISOString");
dateProto.toISOString = function () { return "patched date"; };
console.log(new Date(0).toISOString(),
  Object.getOwnPropertyDescriptor(dateProto, "toISOString").writable,
  Object.getOwnPropertyDescriptor(dateProto, "toISOString").enumerable);
dateProto.toISOString = dateMethod;
console.log(new Date(0).toISOString(), dateDescriptor.writable, dateDescriptor.enumerable);

const typedProto = Object.getPrototypeOf(Uint8Array.prototype);
const typedMethod = typedProto.map;
typedProto.map = function () { return "patched typed array"; };
console.log(new Uint8Array([1]).map((value) => value + 1));
typedProto.map = typedMethod;
console.log([...new Uint8Array([1]).map((value) => value + 1)].join(","));

const oldParseInt = globalThis.parseInt;
globalThis.parseInt = function () { return 37; };
console.log(globalThis.parseInt("5"));
globalThis.parseInt = oldParseInt;
console.log(globalThis.parseInt("5"));
