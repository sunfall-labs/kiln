const value = { polluted: true }
const symbol = Symbol("descriptor")
const source = { ["__proto__"]: value, inheritedSetter: 42, [symbol]: "symbol value" }
let setterCalls = 0
Object.defineProperty(Object.prototype, "inheritedSetter", {
  configurable: true,
  set() { setterCalls += 1 },
})
const descriptors = Object.getOwnPropertyDescriptors(source)
delete Object.prototype.inheritedSetter
const own = Object.getOwnPropertyDescriptor(descriptors, "__proto__")
console.log(Object.getPrototypeOf(descriptors) === Object.prototype)
console.log(Object.hasOwn(descriptors, "__proto__"), own.value.value === value)
console.log(own.writable, own.enumerable, own.configurable)
console.log(Object.hasOwn(descriptors, "inheritedSetter"), descriptors.inheritedSetter.value, setterCalls)
console.log(descriptors[symbol].value, descriptors[symbol].enumerable)
function target() {}
Object.defineProperties(target, descriptors)
console.log(Object.hasOwn(target, "__proto__"), target["__proto__"] === value)
console.log(target.inheritedSetter, target[symbol])
