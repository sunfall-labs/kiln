import * as path from "node:path";

const key = "__kiln_spread_enumerable__";
const hidden = "__kiln_spread_hidden__";
const events = [];
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

const callableCopy = { ...Object };
if (callableCopy[key] !== 17 || events.join(",") !== "receiver" ||
    Object.prototype.hasOwnProperty.call(callableCopy, hidden)) {
  throw new Error("builtin callable spread did not follow own enumerable descriptors");
}
delete Object[key];
delete Object[hidden];

const namespaceCopy = { ...path };
if (namespaceCopy.join !== path.join || namespaceCopy.sep !== path.sep ||
    !Object.prototype.hasOwnProperty.call(namespaceCopy, "join")) {
  throw new Error("builtin namespace spread did not copy enumerable exports");
}
