// Private class elements on a constructor-returned Proxy belong to the Proxy itself. They do not
// forward to the target or invoke public get/defineProperty traps.
const target = {}
const traps = []
const receiver = new Proxy(target, {
  get(target, key, receiver) {
    traps.push(`get:${String(key)}`)
    return Reflect.get(target, key, receiver)
  },
  defineProperty(target, key, descriptor) {
    traps.push(`define:${String(key)}`)
    return Reflect.defineProperty(target, key, descriptor)
  },
  set(_target, key, _value, _receiver) {
    traps.push(`set:${String(key)}`)
    return true
  },
})

class Base {
  constructor() { return receiver }
}
class Derived extends Base {
  #secret = { value: 73 }
  #read() { return this.#secret.value }
  get #value() { return this.#secret.value }
  set #value(value) { this.#secret.value = value }
  readSecret() { return this.#read() }
  readAccessor() { return this.#value }
  writeAccessor(value) { this.#value = value }
  visible = 19
}
const instance = new Derived()
Reflect.set(receiver, "@@private:user-key", 23)
Derived.prototype.writeAccessor.call(instance, 74)
let foreignReceiverError = ""
try {
  Derived.prototype.readSecret.call({})
} catch (error) {
  foreignReceiverError = error.name
}

const revokedTarget = {}
const revoked = Proxy.revocable(revokedTarget, {
  get() { throw new Error("revoked proxy get trap ran") },
  set() { throw new Error("revoked proxy set trap ran") },
})
revoked.revoke()
class RevokedBase {
  constructor() { return revoked.proxy }
}
class RevokedDerived extends RevokedBase {
  #secret = 11
  read() { return this.#secret }
}
const revokedInstance = new RevokedDerived()

console.log(JSON.stringify({
  sameProxy: instance === receiver,
  privateValue: [Derived.prototype.readSecret.call(instance), Derived.prototype.readAccessor.call(instance)],
  traps,
  targetKeys: Reflect.ownKeys(target).map(String),
  revokedPrivate: [revokedInstance === revoked.proxy, RevokedDerived.prototype.read.call(revokedInstance)],
  foreignReceiverError,
}))
