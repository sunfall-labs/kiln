const order: string[] = [];
{
  using first = { [Symbol.dispose]() { order.push("first"); } };
  using second = { [Symbol.dispose]() { order.push("second"); } };
}
if (order.join(",") !== "second,first") throw new Error(`sync disposal order: ${order}`);

let asyncFinished = false;
{
  await using resource = {
    [Symbol.asyncDispose]() {
      return Promise.resolve().then(() => { asyncFinished = true; });
    },
  };
}
if (!asyncFinished) throw new Error("await using did not await async disposal");

let syncFallbackFinished = false;
{
  await using resource = { [Symbol.dispose]() { syncFallbackFinished = true; } };
}
if (!syncFallbackFinished) throw new Error("await using did not use Symbol.dispose fallback");

let nullAsyncFallbackFinished = false;
{
  await using resource = {
    [Symbol.asyncDispose]: null,
    [Symbol.dispose]() { nullAsyncFallbackFinished = true; },
  };
}
if (!nullAsyncFallbackFinished) {
  throw new Error("null Symbol.asyncDispose did not use Symbol.dispose fallback");
}

const disposalOrder: string[] = [];
{
  using syncResource = { [Symbol.dispose]() { disposalOrder.push("sync"); } };
  await using asyncResource = {
    [Symbol.asyncDispose]() {
      return Promise.resolve().then(() => {
        disposalOrder.push("async");
        queueMicrotask(() => {
          disposalOrder.push("queued");
          queueMicrotask(() => disposalOrder.push("nested"));
        });
      });
    },
  };
}
await Promise.resolve();
if (disposalOrder.join(",") !== "async,queued,sync,nested") {
  throw new Error(`async disposal inserted an extra await: ${disposalOrder}`);
}

const errorValue = {};
const suppressedValue = {};
if (SuppressedError.length !== 3 || Object.getPrototypeOf(SuppressedError) !== Error) {
  throw new Error("SuppressedError constructor arity/static prototype mismatch");
}
let messageConversions = 0;
const messageValue = { toString() { messageConversions++; return "message"; } };
const error = new SuppressedError(errorValue, suppressedValue, messageValue);
if (messageConversions !== 1) throw new Error("SuppressedError message conversion count mismatch");
if (!(error instanceof SuppressedError) || !(error instanceof Error)) {
  throw new Error("SuppressedError Error inheritance is missing");
}
if (typeof Error.isError !== "function" || !Error.isError(error)) {
  throw new Error("SuppressedError is missing the engine Error brand");
}
if (Object.prototype.toString.call(error) !== "[object Error]") {
  throw new Error("SuppressedError does not report the Error toStringTag");
}
if (error.error !== errorValue || error.suppressed !== suppressedValue) {
  throw new Error("SuppressedError payloads were not preserved");
}
if (error.toString() !== "SuppressedError: message") throw new Error("SuppressedError toString mismatch");
for (const key of ["error", "suppressed", "message"]) {
  const descriptor = Object.getOwnPropertyDescriptor(error, key);
  if (!descriptor || descriptor.enumerable || !descriptor.writable || !descriptor.configurable) {
    throw new Error(`SuppressedError ${key} descriptor mismatch`);
  }
}

const noMessage = new SuppressedError(errorValue, suppressedValue);
if (noMessage.message !== "" || Object.hasOwn(noMessage, "message")) {
  throw new Error("omitted SuppressedError message should not be an own property");
}
const undefinedMessage = new SuppressedError(errorValue, suppressedValue, undefined);
if (undefinedMessage.message !== "" || Object.hasOwn(undefinedMessage, "message")) {
  throw new Error("undefined SuppressedError message should not be an own property");
}
const nullMessage = new SuppressedError(errorValue, suppressedValue, null);
if (nullMessage.message !== "null" || !Object.hasOwn(nullMessage, "message")) {
  throw new Error("null SuppressedError message conversion mismatch");
}
const extraOptions = new SuppressedError(errorValue, suppressedValue, "", { cause: 1 });
if (Object.hasOwn(extraOptions, "cause")) throw new Error("SuppressedError unexpectedly accepted Error options");
const callReceiver = new SuppressedError("old", "old suppressed", "old");
const callResult = SuppressedError.call(callReceiver, errorValue, suppressedValue, "called");
if (callResult === callReceiver || callResult.message !== "called" || callReceiver.message !== "old") {
  throw new Error("calling SuppressedError must allocate without mutating its receiver");
}
let symbolMessageThrew = false;
try { new SuppressedError(errorValue, suppressedValue, Symbol("message")); }
catch (caught) { symbolMessageThrew = caught instanceof TypeError; }
if (!symbolMessageThrew) throw new Error("SuppressedError must reject a Symbol message");
class DerivedSuppressedError extends SuppressedError {}
const derived = new DerivedSuppressedError(errorValue, suppressedValue);
if (!(derived instanceof DerivedSuppressedError) || !(derived instanceof Error)) {
  throw new Error("derived SuppressedError prototype mismatch");
}
const constructorPrototype = Object.getOwnPropertyDescriptor(SuppressedError, "prototype");
if (!constructorPrototype || constructorPrototype.writable || constructorPrototype.enumerable ||
    constructorPrototype.configurable) {
  throw new Error("SuppressedError constructor prototype descriptor mismatch");
}
const derivedPrototype = {};
let newTargetPrototypeReads = 0;
const newTarget = new Proxy(function NewTarget() {}, {
  get(target, key, receiver) {
    if (key === "prototype") {
      newTargetPrototypeReads++;
      return derivedPrototype;
    }
    return Reflect.get(target, key, receiver);
  },
});
const reflected = Reflect.construct(SuppressedError, [errorValue, suppressedValue], newTarget);
if (newTargetPrototypeReads !== 1 || Object.getPrototypeOf(reflected) !== derivedPrototype) {
  throw new Error("SuppressedError reread newTarget.prototype or lost its selected prototype");
}
console.log("Oxc using context and SuppressedError passed");
