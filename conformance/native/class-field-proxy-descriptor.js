// Class-field definitions on a Proxy must keep exposing the ordinary descriptor object to traps.
const records = []
const receiver = new Proxy({}, {
  defineProperty(target, key, descriptor) {
    records.push([
      typeof key === "symbol" ? key.description : key,
      descriptor.value,
      descriptor.writable,
      descriptor.enumerable,
      descriptor.configurable,
      Object.hasOwn(descriptor, "get"),
      Object.hasOwn(descriptor, "set"),
    ])
    return Reflect.defineProperty(target, key, descriptor)
  },
})

class Base {
  constructor() {
    return receiver
  }
}

class Fields extends Base {
  value = 42;
  [Symbol.for("field")] = 17;
}

new Fields()

let inheritedSetterCalls = 0
class OrdinaryBase {
  set field(_value) { inheritedSetterCalls++ }
  constructor() {
    Object.defineProperty(this, "locked", {
      value: "base", writable: false, enumerable: false, configurable: false,
    })
  }
}
class OrdinaryFields extends OrdinaryBase {
  field = { marker: "kept" };
  __proto__ = "own data";
  [Symbol.for("ordinary-field")] = 29;
}
const ordinary = new OrdinaryFields()
let lockedError = ""
try {
  class Locked extends OrdinaryBase { locked = "replacement" }
  new Locked()
} catch (error) {
  lockedError = error.name
}
console.log(JSON.stringify({
  proxy: records,
  setterCalls: inheritedSetterCalls,
  field: [ordinary.field.marker, Object.hasOwn(ordinary, "field")],
  protoField: [ordinary.__proto__, Object.getPrototypeOf(ordinary) === OrdinaryFields.prototype],
  symbol: Object.getOwnPropertyDescriptor(ordinary, Symbol.for("ordinary-field")),
  lockedError,
}))
