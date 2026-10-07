const own = Symbol("own")
const inherited = Symbol("inherited")
const absent = Symbol("absent")
const date = Object.assign(new Date(0), { [own]: () => "own", named: 42 })
let gets = 0
Object.defineProperty(Date.prototype, inherited, {
  configurable: true,
  get() { gets += 1; return "inherited" },
})
console.log(own in date, date[own](), "named" in date, date.named)
console.log(inherited in date, Reflect.has(date, inherited), gets)
console.log(date[inherited], gets)
console.log(absent in date, "missingDateProperty" in date)
console.log("getTime" in date, Symbol.toPrimitive in date)
delete Date.prototype[inherited]
console.log(inherited in date)
