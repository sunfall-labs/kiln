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
