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
let symbolMessageThrew = false;
try { new SuppressedError(errorValue, suppressedValue, Symbol("message")); }
catch (caught) { symbolMessageThrew = caught instanceof TypeError; }
if (!symbolMessageThrew) throw new Error("SuppressedError must reject a Symbol message");
class DerivedSuppressedError extends SuppressedError {}
const derived = new DerivedSuppressedError(errorValue, suppressedValue);
if (!(derived instanceof DerivedSuppressedError) || !(derived instanceof Error)) {
  throw new Error("derived SuppressedError prototype mismatch");
}
console.log("Oxc using context and SuppressedError passed");
