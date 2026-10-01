import * as path from "node:path";

const key = "__kiln_spread_enumerable__";
const hidden = "__kiln_spread_hidden__";
const symbolKey = Symbol("kiln spread enumerable");
const hiddenSymbolKey = Symbol("kiln spread hidden");
const events = [];
Object.defineProperty(Object.prototype, key, {
  configurable: true,
  set() {
    events.push("inherited setter");
  },
});
Object.defineProperty(Object, key, {
  configurable: true,
  enumerable: true,
  get() {
    events.push(this === Object ? "receiver" : "wrong receiver");
    return 17;
  },
});
Object.defineProperty(Object, hidden, {
  configurable: true,
  enumerable: false,
  value: 19,
});
Object.defineProperty(Object.prototype, symbolKey, {
  configurable: true,
  set() {
    events.push("inherited symbol setter");
  },
});
Object.defineProperty(Object, symbolKey, {
  configurable: true,
  enumerable: true,
  get() {
    events.push(this === Object ? "symbol receiver" : "wrong symbol receiver");
    return 23;
  },
});
Object.defineProperty(Object, hiddenSymbolKey, {
  configurable: true,
  enumerable: false,
  value: 29,
});

const callableCopy = { ...Object };
if (callableCopy[key] !== 17 || callableCopy[symbolKey] !== 23 ||
    events.join(",") !== "receiver,symbol receiver" ||
    Object.prototype.hasOwnProperty.call(callableCopy, hidden) ||
    !Object.prototype.hasOwnProperty.call(callableCopy, key) ||
    !Object.prototype.hasOwnProperty.call(callableCopy, symbolKey) ||
    Object.prototype.hasOwnProperty.call(callableCopy, hiddenSymbolKey)) {
  throw new Error("builtin callable spread did not follow own enumerable descriptors");
}
Object.defineProperty(Object, "__kiln_spread_throws__", {
  configurable: true,
  enumerable: true,
  get() { throw new Error("spread getter"); },
});
let getterThrew = false;
try {
  ({ ...Object });
} catch (error) {
  getterThrew = error.message === "spread getter";
}
if (!getterThrew) throw new Error("builtin spread swallowed a throwing getter");
delete Object[key];
delete Object[hidden];
delete Object[symbolKey];
delete Object[hiddenSymbolKey];
delete Object.__kiln_spread_throws__;
delete Object.prototype[key];
delete Object.prototype[symbolKey];

const namespaceCopy = { ...path };
if (namespaceCopy.join !== path.join || namespaceCopy.sep !== path.sep ||
    !Object.prototype.hasOwnProperty.call(namespaceCopy, "join")) {
  throw new Error("builtin namespace spread did not copy enumerable exports");
}
