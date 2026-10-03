const events = []
const values = ["unused"]
Object.defineProperty(values, "0", {
  configurable: true,
  enumerable: true,
  get() {
    events.push(this === values ? "getter-this" : "wrong-this")
    return 10
  },
  set(value) {
    events.push(this === values ? `setter:${value}` : "wrong-this")
  }
})
if (values[0] !== 10 || events.join(",") !== "getter-this") {
  throw new Error(`indexed getter/receiver: ${events}`)
}
if (!(0 in values) || Reflect.ownKeys(values).join(",") !== "0,length") {
  throw new Error(`indexed own keys: ${Reflect.ownKeys(values)}`)
}
const descriptor = Object.getOwnPropertyDescriptor(values, "0")
if (descriptor.enumerable !== true || descriptor.configurable !== true ||
    typeof descriptor.get !== "function" || typeof descriptor.set !== "function") {
  throw new Error("indexed accessor descriptor was not preserved")
}
values[0] = 12
if (events.join(",") !== "getter-this,setter:12") {
  throw new Error(`indexed setter/receiver: ${events}`)
}
const mapped = values.map(value => value * 2)
if (mapped[0] !== 20 || events.join(",") !== "getter-this,setter:12,getter-this") {
  throw new Error(`map did not use indexed [[Get]] once: ${events}`)
}
if (!delete values[0] || 0 in values || values.length !== 1) {
  throw new Error("deleting an indexed accessor did not leave a hole")
}

const visits = []
const indexed = ["a", "b", "c"]
Object.defineProperty(indexed, "1", {
  configurable: true,
  enumerable: true,
  get() {
    visits.push("get:1")
    return "B"
  },
})
if (indexed.reduce((all, value) => all + value, "") !== "aBc" ||
    indexed.slice().join("") !== "aBc" || indexed.includes("B") !== true ||
    indexed.indexOf("B") !== 1) {
  throw new Error("array algorithms bypassed indexed [[Get]]")
}
if (visits.join(",") !== "get:1,get:1,get:1,get:1") {
  throw new Error(`array algorithm getter count/order: ${visits}`)
}
Object.freeze(indexed)
const frozenIndex = Object.getOwnPropertyDescriptor(indexed, "1")
if (Object.isFrozen(indexed) !== true || frozenIndex.configurable !== false ||
    Object.getOwnPropertyDescriptor(indexed, "0").writable !== false) {
  throw new Error("freezing did not preserve indexed descriptors")
}
if (Reflect.set(indexed, "0", "changed") !== false || indexed[0] !== "a") {
  throw new Error("frozen dense element was writable")
}
const popEvents = []
const popTarget = [0]
Object.defineProperty(popTarget, "0", {
  configurable: true,
  get() {
    popEvents.push("get")
    return 42
  },
})
const popped = popTarget.pop()
if (popped !== 42 || popTarget.length !== 0 || 0 in popTarget || popEvents.join(",") !== "get") {
  throw new Error(`pop accessor behavior: ${popped}/${popEvents}/${popTarget.length}/${0 in popTarget}`)
}
console.log("array indexed accessors ok")
