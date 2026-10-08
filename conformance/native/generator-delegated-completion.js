function* outer_throw(delegate) {
  try {
    yield "body";
  } finally {
    const value = yield* delegate;
    console.log("delegate-value", value);
  }
  return "after-finally";
}

async function run_throw() {
  const delegatedError = { kind: "delegated throw" };
  const originalReturn = { kind: "original return" };
  const delegate = {
    [Symbol.iterator]() { return this; },
    next() { return { value: "delegate-yield", done: false }; },
    throw(error) {
      console.log("delegate-throw", error === delegatedError);
      return { value: "delegate-final", done: true };
    },
  };
  const iterator = outer_throw(delegate);
  console.log("first", JSON.stringify(iterator.next()));
  console.log("return", JSON.stringify(iterator.return(originalReturn)));
  try {
    const result = iterator.throw(delegatedError);
    console.log("throw-result", result.value === originalReturn, result.done,
      result.value === delegatedError);
  } catch (error) {
    console.log("host-caught", error === delegatedError,
      error === originalReturn, String(error));
  }
  console.log("finished", JSON.stringify(iterator.next()));
}
run_throw().catch(error => { console.log("host-rejected", String(error)); });

function* outer_return(delegate) {
  try { yield "body"; }
  finally { const value = yield* delegate; console.log("delegate-value", value); }
}
async function run_return() {
  const first = { marker: "first return" };
  const second = { marker: "second return" };
  const delegate = {
    [Symbol.iterator]() { return this; },
    next() { return { value: "delegate-yield", done: false }; },
    return(value) {
      console.log("delegate-return", value === second);
      return { value: "delegate-returned", done: true };
    },
  };
  const iterator = outer_return(delegate);
  console.log("first", JSON.stringify(iterator.next()));
  console.log("return-one", JSON.stringify(iterator.return(first)));
  try {
    const result = iterator.return(second);
    console.log("return-two", result.value === "delegate-returned", result.done,
      result.value === first, result.value === second);
  } catch (error) {
    console.log("host-caught", String(error), error === first, error === second);
  }
  console.log("finished", JSON.stringify(iterator.next()));
}
run_return().catch(error => console.log("host-rejected", String(error)));

function* outer_throw_from_body(delegate) {
  try { yield "body"; }
  finally { yield* delegate; }
}
const originalError = { marker: "outer throw" };
const delegatedStep = { value: "delegated", done: false };
let throwDelegateCalls = 0;
const throwDelegate = {
  [Symbol.iterator]() { return this; },
  next() {
    throwDelegateCalls += 1;
    return throwDelegateCalls === 1 ? delegatedStep : { value: undefined, done: true };
  },
};
const throwing = outer_throw_from_body(throwDelegate);
throwing.next();
const delegatedThrowResult = throwing.throw(originalError);
console.log("throw-finally-yield", delegatedThrowResult === delegatedStep,
  delegatedThrowResult.value === "delegated", delegatedThrowResult.done);
try { throwing.next(); }
catch (error) { console.log("throw-finally-completion", error === originalError); }

function* outer_return_across_delegates(first, second) {
  try { yield "body"; }
  finally { yield* first; yield* second; }
}
const originalReturnValue = { marker: "outer return" };
const delegateThrowError = { marker: "delegate throw" };
const firstDelegate = {
  [Symbol.iterator]() { return this; },
  next() { return { value: "first delegate", done: false }; },
  throw() { return { value: "first done", done: true }; },
};
const secondStep = { value: "second delegate", done: false };
let secondDelegateCalls = 0;
const secondDelegate = {
  [Symbol.iterator]() { return this; },
  next() {
    secondDelegateCalls += 1;
    return secondDelegateCalls === 1 ? secondStep : { value: undefined, done: true };
  },
};
const returning = outer_return_across_delegates(firstDelegate, secondDelegate);
returning.next();
const firstReturnResult = returning.return(originalReturnValue);
const secondDelegateResult = returning.throw(delegateThrowError);
console.log("return-cross-delegate-yield", firstReturnResult.value === "first delegate",
  secondDelegateResult === secondStep, secondDelegateResult.done);
const completedReturn = returning.next();
console.log("return-cross-delegate-completion", completedReturn.value === originalReturnValue,
  completedReturn.done);
