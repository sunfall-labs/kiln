function userCode() {
  return new Error("boom");
}

const error = userCode();
if (Error.isError(error) !== true || Error.isError({ name: "Error", message: "boom" }) !== false ||
    Error.isError(new Proxy(error, {})) !== false) {
  throw new Error("Error.isError did not use the Error brand");
}
const stackDescriptor = Object.getOwnPropertyDescriptor(error, "stack");
if (typeof Bun === "undefined" &&
    (!stackDescriptor || typeof stackDescriptor.get !== "function" ||
      typeof stackDescriptor.set !== "function" || stackDescriptor.enumerable ||
      !stackDescriptor.configurable || Object.getOwnPropertyNames(error)[0] !== "stack" ||
      Object.getOwnPropertyDescriptor(Error.prototype, "stack") !== undefined ||
      Error.prototype.stack !== undefined)) {
  throw new Error("Node-compatible Error stack accessor has the wrong own-property shape");
}
if (typeof stackDescriptor?.get === "function" &&
    (stackDescriptor.get.call({}) !== undefined || stackDescriptor.get.call(Error.prototype) !== undefined)) {
  throw new Error("borrowed Error stack getter exposed frames for a non-Error");
}
if (Reflect.ownKeys(error).some((key) => typeof key === "string" && key.startsWith("@@")) ||
    Object.getOwnPropertyNames(error).some((key) => key.startsWith("@@")) ||
    Object.getOwnPropertySymbols(error).length !== 0) {
  throw new Error("automatic stack metadata leaked through own-key reflection");
}
if (typeof error.stack !== "string" ||
    (typeof Bun === "undefined" && !error.stack.includes("userCode"))) {
  throw new Error(`new Error omitted the user frame: ${error.stack}`);
}

let prepareCalls = 0;
Error.prepareStackTrace = (thrown, sites) => {
  prepareCalls++;
  return `${thrown.message}:${sites[0]?.getFunctionName()}`;
};
function lazyError() {
  return new TypeError("lazy");
}
const lazy = lazyError();
if (prepareCalls !== 0 || !lazy.stack.startsWith("lazy:") || prepareCalls !== 1 ||
    !lazy.stack.startsWith("lazy:") || prepareCalls !== 1) {
  throw new Error(`Error.prepareStackTrace was not lazy: ${prepareCalls}, ${lazy.stack}`);
}
Error.prepareStackTrace = undefined;

const savedStackTraceLimit = Error.stackTraceLimit;
const isBun = typeof Bun !== "undefined";
Error.stackTraceLimit = 0;
function zeroFrameError() { return new Error("zero"); }
if (!isBun && zeroFrameError().stack !== "Error: zero") {
  throw new Error("zero Error.stackTraceLimit must retain the stack header");
}
for (const disabledLimit of [undefined, "2"]) {
  Error.stackTraceLimit = disabledLimit;
  if (!isBun && new Error("disabled").stack !== undefined) {
    throw new Error("non-numeric Error.stackTraceLimit must disable stack capture");
  }
}
for (const emptyLimit of [NaN, -1]) {
  Error.stackTraceLimit = emptyLimit;
  if (!isBun && new Error("empty").stack !== "Error: empty") {
    throw new Error("negative or NaN Error.stackTraceLimit must keep the stack header");
  }
}
Error.stackTraceLimit = savedStackTraceLimit;

class CustomError extends Error {
  constructor() {
    super("custom");
  }
}
function makeCustomError() {
  return new CustomError();
}
if (!makeCustomError().stack.includes("makeCustomError")) {
  throw new Error("subclass construction lost its caller frame");
}
class CustomTypeError extends TypeError {
  constructor() { super("subtype"); }
}
const customTypeError = new CustomTypeError();
if (Object.getPrototypeOf(customTypeError) !== CustomTypeError.prototype ||
    Object.getPrototypeOf(CustomTypeError.prototype) !== TypeError.prototype ||
    customTypeError.constructor !== CustomTypeError || !(customTypeError instanceof TypeError)) {
  throw new Error("derived TypeError lost its prototype chain");
}

const constructors = [Error, EvalError, RangeError, ReferenceError, SyntaxError, TypeError, URIError, AggregateError];
for (const Constructor of constructors) {
  const instance = Constructor === AggregateError ? new Constructor([]) : new Constructor("prototype");
  if (Object.getPrototypeOf(instance) !== Constructor.prototype || instance.constructor !== Constructor) {
    throw new Error(`${Constructor.name} construction used the wrong prototype`);
  }
}
const reflected = Reflect.construct(Error, ["reflected"], TypeError);
if (Object.getPrototypeOf(reflected) !== TypeError.prototype || reflected.constructor !== TypeError) {
  throw new Error("Reflect.construct lost the requested Error prototype");
}

console.log("automatic error stack passed");
