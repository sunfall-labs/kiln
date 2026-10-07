// Private brand membership is an own private-slot query. It does not delegate to Proxy `has`.
const events = []
let proxy
const target = {}
proxy = new Proxy(target, {
  has(_target, key) {
    events.push(`has:${String(key)}`)
    return true
  },
})

class Base {
  constructor(receiver) { return receiver }
}
class Derived extends Base {
  #ready = false
  initializedBeforeLater = #later in this
  #method() {}
  #accessCount = 0
  get #hidden() { this.#accessCount += 1; return 7 }
  methodPresentBeforeField = #method in this
  #later = 5
  #readyAfter = this.#ready
  check(value) { return #later in value }
  methodInstalled(value) { return #method in value }
  accessorInstalled(value) { return #hidden in value }
  accessorCount() { return this.#accessCount }
  ready(value) { return #ready in value }
  later(value) { return #later in value }
  static #staticValue = 1
  static staticCheck(value) { return #staticValue in value }
}
const instance = new Derived(proxy)
const foreign = {}
const targetHasPrivate = Derived.prototype.check.call(instance, target)
const proxyHasPrivate = Derived.prototype.check.call(instance, proxy)
const methodHasPrivate = Derived.prototype.methodInstalled.call(instance, proxy)
const accessorHasPrivate = Derived.prototype.accessorInstalled.call(instance, proxy)
const accessorCalls = Derived.prototype.accessorCount.call(instance)
const initialized = Derived.prototype.later.call(instance, proxy)
const readyField = Derived.prototype.ready.call(instance, proxy)
const staticInherited = Derived.staticCheck(Object.create(Derived))

function makeClass() {
  return class Repeated {
    #value = 1
    static has(value) { return #value in value }
  }
}
const First = makeClass()
const Second = makeClass()
const first = new First()
const second = new Second()
const distinctEvaluation = [First.has(first), First.has(second)]

const revokedTarget = {}
const revocable = Proxy.revocable(revokedTarget, { has() { throw new Error("has trap ran") } })
class RevokedBase { constructor() { return revocable.proxy } }
class RevokedDerived extends RevokedBase {
  #secret = 1
  has(value) { return #secret in value }
}
const revokedInstance = new RevokedDerived()
revocable.revoke()
const revokedPrivate = RevokedDerived.prototype.has.call(revokedInstance, revokedInstance)

let primitiveError = ""
try { Derived.prototype.check.call(instance, null) } catch (error) { primitiveError = error.name }

console.log(JSON.stringify({
  targetHasPrivate,
  proxyHasPrivate,
  methodHasPrivate,
  accessorHasPrivate,
  accessorCalls,
  methodPresentBeforeField: instance.methodPresentBeforeField,
  initializedBeforeLater: instance.initializedBeforeLater,
  initialized,
  readyField,
  staticInherited,
  distinctEvaluation,
  revokedPrivate,
  primitiveError,
  events,
  foreign: Derived.prototype.check.call(instance, foreign),
}))
