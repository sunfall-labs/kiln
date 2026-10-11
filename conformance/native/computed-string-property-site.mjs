const evaluate = "~effect/Effect/evaluate"
const args = "~effect/Effect/args"
const contA = "~effect/Effect/successCont"

function readEffectKey(object, key) {
  return object[key]
}
function readAlpha(object) {
  return object.alpha
}
function readInherited(object) {
  return object.inherited
}
function readDomain(object, key) {
  return object[key]
}
function readLocal(object, key) {
  return object[key]
}
function readDescriptor(object) {
  return object.mutable
}
function readDictionary(object) {
  return object.alpha
}
function readIndexed(object, key) {
  return object[key]
}

const primitive = {
  [evaluate]: "run",
  [args]: 2,
  [contA]: "continue",
  alpha: 1,
  beta: 10
}
const results = []
results.push(readEffectKey(primitive, evaluate), readEffectKey(primitive, evaluate))
results.push(readEffectKey(primitive, args), readEffectKey(primitive, contA))
results.push(readAlpha(primitive), readAlpha(primitive))
primitive.alpha = 3
results.push(readAlpha(primitive))
primitive.gamma = 30
results.push(readDomain(primitive, "gamma"), readDomain(primitive, "gamma"))
results.push(readDomain(primitive, "beta"))

const firstPrototype = { inherited: "first" }
const secondPrototype = { inherited: "second" }
const prototyped = Object.create(firstPrototype)
results.push(readInherited(prototyped))
Object.setPrototypeOf(prototyped, secondPrototype)
results.push(readInherited(prototyped))
const initiallyAbsent = Object.create(null)
results.push(readInherited(initiallyAbsent))
Object.defineProperty(initiallyAbsent, "inherited", { value: "present", configurable: true })
results.push(readInherited(initiallyAbsent))

const descriptorTarget = { mutable: "data" }
results.push(readDescriptor(descriptorTarget))
let getterCalls = 0
Object.defineProperty(descriptorTarget, "mutable", {
  configurable: true,
  get() {
    return ++getterCalls
  }
})
results.push(readDescriptor(descriptorTarget), readDescriptor(descriptorTarget), getterCalls)

let proxyGets = 0
const proxied = new Proxy({ alpha: "target" }, {
  get(target, key, receiver) {
    if (key === "alpha") proxyGets++
    return Reflect.get(target, key, receiver)
  }
})
results.push(readAlpha(proxied), readAlpha(proxied), proxyGets)

const dictionary = Object.create(null)
for (let index = 0; index < 40; index++) dictionary[`field${index}`] = index
dictionary.alpha = "dictionary"
results.push(readDictionary(dictionary), readDictionary(dictionary))

const keyedBothWays = {
  "@@iterator": "string-key",
  [Symbol.iterator]: "symbol-key"
}
results.push(readDomain(keyedBothWays, "@@iterator"), readDomain(keyedBothWays, Symbol.iterator))

const indexed = ["zero", "one"]
indexed["01"] = "named-zero-one"
results.push(readIndexed(indexed, "1"), readIndexed(indexed, "01"))

/* A runtime-created non-index key exercises the realm-local interned-key retirement path. */
const localKey = ["runtime", "only", "key"].join("/")
primitive[localKey] = "local"
results.push(readLocal(primitive, localKey), readLocal(primitive, localKey))

console.log(results.join("|"))
