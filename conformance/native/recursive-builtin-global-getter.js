const originalDate = Date
let calls = 0
Object.defineProperty(globalThis, "Date", {
  configurable: true,
  get() {
    calls += 1
    return Date
  },
})
let bounded = false
try {
  Date
} catch (error) {
  bounded = error instanceof RangeError
}
Object.defineProperty(globalThis, "Date", {
  value: originalDate,
  writable: true,
  configurable: true,
})
console.log(bounded && calls > 1)
