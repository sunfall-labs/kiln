function check(condition, message) {
  if (!condition) throw new Error(message)
}

let delegatedGetterReads = 0
let delegatedCalls = 0
const delegated = {
  [Symbol.iterator]() {
    return this
  },
  get next() {
    delegatedGetterReads++
    return function (sent) {
      check(this === delegated, "yield* preserves the iterator receiver")
      check(arguments.length === 1, "yield* passes exactly one argument")
      delegatedCalls++
      if (delegatedCalls === 1) return { value: "first", done: false }
      return { value: sent, done: true }
    }
  },
  return(value) {
    return { value, done: true }
  },
  throw(value) {
    return { value: `caught:${value}`, done: true }
  }
}
const outer = (function* () {
  return yield* delegated
})()
const yielded = outer.next()
check(yielded.value === "first" && yielded.done === false, "delegated yield value")
Object.defineProperty(delegated, "next", {
  configurable: true,
  value() {
    throw new Error("yield* reread next")
  }
})
const completed = outer.next("sent")
check(completed.value === "sent" && completed.done === true, "captured next survives mutation")
check(delegatedGetterReads === 1 && delegatedCalls === 2, "yield* reads next once")

const returnDelegate = {
  [Symbol.iterator]() { return this },
  next() { return { value: "yielded", done: false } },
  return(value) {
    check(this === returnDelegate, "yield* return preserves the iterator receiver")
    return { value, done: true }
  }
}
const returningOuter = (function* () { return yield* returnDelegate })()
returningOuter.next()
const returned = returningOuter.return("closed")
check(returned.value === "closed" && returned.done === true, "yield* forwards return")

const throwDelegate = {
  [Symbol.iterator]() { return this },
  next() { return { value: "yielded", done: false } },
  throw(value) {
    check(this === throwDelegate, "yield* throw preserves the iterator receiver")
    return { value: `caught:${value}`, done: true }
  }
}
const throwingOuter = (function* () { return yield* throwDelegate })()
throwingOuter.next()
const thrown = throwingOuter.throw("problem")
check(thrown.value === "caught:problem" && thrown.done === true, "yield* forwards throw")

let forOfGetterReads = 0
let forOfCalls = 0
const forOfIterable = {
  [Symbol.iterator]() { return this },
  get next() {
    forOfGetterReads++
    return function () {
      check(this === forOfIterable, "for-of preserves the iterator receiver")
      check(arguments.length === 0, "for-of passes no arguments")
      forOfCalls++
      return forOfCalls === 1
        ? { value: "for-of", done: false }
        : { value: undefined, done: true }
    }
  }
}
let forOfValue
for (forOfValue of forOfIterable) {}
check(forOfValue === "for-of", "for-of receives the iterator value")
check(forOfGetterReads === 1 && forOfCalls === 2, "for-of captures next once")

let destructureGetterReads = 0
let destructureCalls = 0
let closeCalls = 0
const destructureIterable = {
  [Symbol.iterator]() { return this },
  get next() {
    destructureGetterReads++
    return function () {
      check(this === destructureIterable, "destructuring preserves the iterator receiver")
      check(arguments.length === 0, "destructuring passes no arguments")
      destructureCalls++
      return { value: "destructured", done: false }
    }
  },
  return() {
    closeCalls++
    return { value: undefined, done: true }
  }
}
const [destructured] = destructureIterable
check(destructured === "destructured", "destructuring receives the iterator value")
check(destructureGetterReads === 1 && destructureCalls === 1, "destructuring captures next once")
check(closeCalls === 1, "destructuring closes the unfinished iterator")

console.log("prepared iterator next calls pass")
