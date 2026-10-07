// Large in-range hole fills use dense storage only when the indexed prototype chain and
// receiver are ordinary. The other cases still go through observable [[Set]].
const dense = new Array(262144)
const filled = dense.fill(37, 1, dense.length - 1)
console.log(
  "dense",
  filled === dense,
  dense.length,
  0 in dense,
  dense[1],
  dense[131072],
  dense[dense.length - 2],
  dense.length - 1 in dense
)

const ordinary = new Array(4)
const customPrototype = Object.create(Array.prototype)
let setterCalls = 0
let setterReceiver = false
Object.defineProperty(customPrototype, "2", {
  configurable: true,
  set(value) {
    setterCalls++
    setterReceiver = this === ordinary && value === 9
  }
})
Object.setPrototypeOf(ordinary, customPrototype)
ordinary.fill(9)
console.log(
  "custom-setter",
  setterCalls,
  setterReceiver,
  Object.prototype.hasOwnProperty.call(ordinary, "2"),
  ordinary[0],
  ordinary[3]
)

const converted = new Array(3)
const convertedPrototype = Object.create(Array.prototype)
let conversionSetterCalls = 0
Object.defineProperty(convertedPrototype, "1", {
  configurable: true,
  set() {
    conversionSetterCalls++
  }
})
const end = {
  valueOf() {
    Object.setPrototypeOf(converted, convertedPrototype)
    return 3
  }
}
converted.fill(6, 0, end)
console.log(
  "coercion-before-guard",
  conversionSetterCalls,
  Object.prototype.hasOwnProperty.call(converted, "1"),
  converted[0],
  converted[2]
)

// The intrinsic-chain guards are monotonic and are set when the default Array.prototype or
// Object.prototype gains an index. Mutating either chain during ToIntegerOrInfinity must force
// the generic Set path for this fill.
const arrayIndexDescriptor = Object.getOwnPropertyDescriptor(Array.prototype, "1")
const arrayPrototypeReceiver = new Array(3)
let arrayPrototypeSetterCalls = 0
let arrayPrototypeSetterReceiver = false
const arrayPrototypeEnd = {
  valueOf() {
    Object.defineProperty(Array.prototype, "1", {
      configurable: true,
      set(value) {
        arrayPrototypeSetterCalls++
        arrayPrototypeSetterReceiver = this === arrayPrototypeReceiver && value === 12
      }
    })
    return 3
  }
}
try {
  arrayPrototypeReceiver.fill(12, 0, arrayPrototypeEnd)
} finally {
  if (arrayIndexDescriptor) Object.defineProperty(Array.prototype, "1", arrayIndexDescriptor)
  else delete Array.prototype["1"]
}
console.log(
  "intrinsic-array-prototype-setter",
  arrayPrototypeSetterCalls,
  arrayPrototypeSetterReceiver,
  Object.prototype.hasOwnProperty.call(arrayPrototypeReceiver, "1"),
  arrayPrototypeReceiver[0],
  arrayPrototypeReceiver[2]
)

const objectIndexDescriptor = Object.getOwnPropertyDescriptor(Object.prototype, "2")
const objectPrototypeReceiver = new Array(4)
let objectPrototypeSetterCalls = 0
let objectPrototypeSetterReceiver = false
const objectPrototypeEnd = {
  valueOf() {
    Object.defineProperty(Object.prototype, "2", {
      configurable: true,
      set(value) {
        objectPrototypeSetterCalls++
        objectPrototypeSetterReceiver = this === objectPrototypeReceiver && value === 13
      }
    })
    return 4
  }
}
try {
  objectPrototypeReceiver.fill(13, 0, objectPrototypeEnd)
} finally {
  if (objectIndexDescriptor) Object.defineProperty(Object.prototype, "2", objectIndexDescriptor)
  else delete Object.prototype["2"]
}
console.log(
  "intrinsic-object-prototype-setter",
  objectPrototypeSetterCalls,
  objectPrototypeSetterReceiver,
  Object.prototype.hasOwnProperty.call(objectPrototypeReceiver, "2"),
  objectPrototypeReceiver[0],
  objectPrototypeReceiver[3]
)

const originalArrayPrototypeParent = Object.getPrototypeOf(Array.prototype)
const inheritedArrayIndexParent = Object.create(null)
const inheritedArrayReceiver = new Array(4)
let inheritedArraySetterCalls = 0
let inheritedArraySetterReceiver = false
Object.defineProperty(inheritedArrayIndexParent, "3", {
  configurable: true,
  set(value) {
    inheritedArraySetterCalls++
    inheritedArraySetterReceiver = this === inheritedArrayReceiver && value === 14
  }
})
const inheritedArrayEnd = {
  valueOf() {
    Object.setPrototypeOf(Array.prototype, inheritedArrayIndexParent)
    return 4
  }
}
try {
  inheritedArrayReceiver.fill(14, 0, inheritedArrayEnd)
} finally {
  Object.setPrototypeOf(Array.prototype, originalArrayPrototypeParent)
}
console.log(
  "inherited-array-prototype-setter",
  inheritedArraySetterCalls,
  inheritedArraySetterReceiver,
  Object.prototype.hasOwnProperty.call(inheritedArrayReceiver, "3"),
  inheritedArrayReceiver[0],
  inheritedArrayReceiver[2]
)

const readonlyArrayParent = Object.create(null)
const readonlyArrayReceiver = new Array(4)
Object.defineProperty(readonlyArrayParent, "2", {
  configurable: true,
  value: 21,
  writable: false
})
let readonlyArrayRefused = false
try {
  Object.setPrototypeOf(Array.prototype, readonlyArrayParent)
  readonlyArrayReceiver.fill(15)
} catch (error) {
  readonlyArrayRefused = error instanceof TypeError
} finally {
  Object.setPrototypeOf(Array.prototype, originalArrayPrototypeParent)
}
console.log(
  "inherited-readonly-array-index",
  readonlyArrayRefused,
  readonlyArrayReceiver[0],
  readonlyArrayReceiver[1],
  Object.prototype.hasOwnProperty.call(readonlyArrayReceiver, "2"),
  Object.prototype.hasOwnProperty.call(readonlyArrayReceiver, "3")
)

const getterOnlyArrayParent = Object.create(null)
const getterOnlyArrayReceiver = new Array(4)
let getterOnlyCalls = 0
Object.defineProperty(getterOnlyArrayParent, "2", {
  configurable: true,
  get() {
    getterOnlyCalls++
    return 22
  }
})
let getterOnlyArrayRefused = false
try {
  Object.setPrototypeOf(Array.prototype, getterOnlyArrayParent)
  getterOnlyArrayReceiver.fill(16)
} catch (error) {
  getterOnlyArrayRefused = error instanceof TypeError
} finally {
  Object.setPrototypeOf(Array.prototype, originalArrayPrototypeParent)
}
console.log(
  "inherited-getter-only-array-index",
  getterOnlyArrayRefused,
  getterOnlyCalls,
  getterOnlyArrayReceiver[0],
  getterOnlyArrayReceiver[1],
  Object.prototype.hasOwnProperty.call(getterOnlyArrayReceiver, "2"),
  Object.prototype.hasOwnProperty.call(getterOnlyArrayReceiver, "3")
)

const nonExtensibleSetter = new Array(1)
let nonExtensibleCalls = 0
const setterPrototype = Object.create(Array.prototype)
Object.defineProperty(setterPrototype, "0", {
  configurable: true,
  set(value) {
    nonExtensibleCalls++
    if (this !== nonExtensibleSetter || value !== 11) throw new Error("bad receiver")
  }
})
Object.setPrototypeOf(nonExtensibleSetter, setterPrototype)
Object.preventExtensions(nonExtensibleSetter)
nonExtensibleSetter.fill(11)
console.log(
  "nonextensible-inherited-setter",
  nonExtensibleCalls,
  Object.prototype.hasOwnProperty.call(nonExtensibleSetter, "0")
)

const nonExtensibleHole = new Array(2)
Object.preventExtensions(nonExtensibleHole)
let refused = false
try {
  nonExtensibleHole.fill(5, 0, 1)
} catch (error) {
  refused = error instanceof TypeError
}
console.log(
  "nonextensible-hole",
  refused,
  Object.prototype.hasOwnProperty.call(nonExtensibleHole, "0")
)
