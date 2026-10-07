const originalDate = Date
const originalMath = Math
const originalSetTimeout = setTimeout
const originalTypeError = TypeError

globalThis.TypeError = function ReplacementTypeError() {}
let internalTypeErrorStayedIntrinsic = false
try {
  ;({} = null)
} catch (error) {
  internalTypeErrorStayedIntrinsic = error instanceof originalTypeError
}
if (!internalTypeErrorStayedIntrinsic) {
  throw new Error("compiler-generated TypeError was redirected through the mutable global")
}
globalThis.TypeError = originalTypeError

const fakeTimeout = () => "replacement timer"
globalThis.setTimeout = fakeTimeout
if (setTimeout !== fakeTimeout || setTimeout() !== "replacement timer") {
  throw new Error("source setTimeout bypassed its mutable global binding")
}

globalThis.Date = { now: () => 123 }
if (Date.now() !== 123) {
  throw new Error("source Date bypassed its mutable global binding")
}

globalThis.Math = { floor: () => "replacement" }
if (Math.floor(2) !== "replacement") {
  throw new Error("source Math bypassed its mutable global binding")
}

const originalArray = Array
const replacementArrayResult = { marker: "replacement Array" }
globalThis.Array = function () { return replacementArrayResult }
if (new Array() !== replacementArrayResult) {
  throw new Error("source Array constructor bypassed its mutable global binding")
}

let arraysInstalledByMathGetter = 0
Object.defineProperty(globalThis, "Math", {
  configurable: true,
  get() {
    arraysInstalledByMathGetter += 1
    globalThis.Array = function () { return replacementArrayResult }
    return { floor: () => "getter" }
  },
})
if (Math.floor(2) !== "getter" || new Array() !== replacementArrayResult || arraysInstalledByMathGetter !== 1) {
  throw new Error("global accessor effects were not observed by later builtin reads")
}

let dateGets = 0
Object.defineProperty(globalThis, "Date", {
  configurable: true,
  get() {
    dateGets += 1
    return { now: () => dateGets }
  },
})
if (Date.now() !== 1 || Date.now() !== 2 || dateGets !== 2) {
  throw new Error(`source Date did not perform each global accessor read: ${dateGets}`)
}

let discardedDateGets = 0
Object.defineProperty(globalThis, "Date", {
  configurable: true,
  get() {
    discardedDateGets += 1
    return originalDate
  },
})
Date
if (discardedDateGets !== 1) {
  throw new Error(`discarded global read skipped its accessor: ${discardedDateGets}`)
}

delete globalThis.Date
let missingDateThrew = false
try {
  Date
} catch (error) {
  missingDateThrew = error.name === "ReferenceError"
}
if (!missingDateThrew) {
  throw new Error("deleted builtin binding did not throw ReferenceError")
}

globalThis.Date = originalDate
Object.defineProperty(globalThis, "Math", {
  value: originalMath,
  writable: true,
  enumerable: false,
  configurable: true,
})
globalThis.Array = originalArray
globalThis.setTimeout = originalSetTimeout
if (Date !== originalDate || Math !== originalMath || setTimeout !== originalSetTimeout) {
  throw new Error("reinstalled source builtin bindings were not observed")
}
console.log("replaced builtin global bindings ok")
