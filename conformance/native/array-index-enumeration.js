// Dense elements and indexed descriptor overlays name the same property exactly once.
const mapped = [3, 4, 5].map((value) => value * 2)
const copied = Array.from(mapped)
const filtered = mapped.filter((value) => value > 6)
console.log(JSON.stringify([mapped, Object.keys(mapped), Object.keys(copied), Object.keys(filtered)]))

let getterCalls = 0
const array = [10, 20, 30]
Object.defineProperty(array, "0", {
  value: 11, writable: true, enumerable: true, configurable: true,
})
Object.defineProperty(array, "1", { enumerable: false })
delete array[2]
Object.defineProperty(array, "2", {
  get() { getterCalls++; return 31 }, enumerable: true, configurable: true,
})
array.named = "kept"
array["01"] = "named numeric spelling"
array["4294967295"] = "outside index range"
array[Symbol.for("extra")] = "symbol"
console.log(JSON.stringify([Object.keys(array), Reflect.ownKeys(array).map((key) =>
  typeof key === "symbol" ? `symbol:${key.description}` : key), getterCalls]))
delete array[0]
Object.defineProperty(array, "0", {
  value: 12, writable: true, enumerable: true, configurable: true,
})
console.log(JSON.stringify([Object.keys(array), getterCalls]))
