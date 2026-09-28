// A fixed runtime spelling can also be an ordinary user string. Registering bootstrap keys
// process-wide must preserve the separate string, symbol and private property domains.
const value = {
  "@@dispose": "string",
  "@@node:readable": "user string",
  [Symbol.dispose]: "symbol",
};
console.log(value["@@dispose"], value["@@node:readable"], value[Symbol.dispose]);
console.log(Object.getOwnPropertyNames(value).join(","));
console.log(Reflect.ownKeys(value).map(String).join(","));
