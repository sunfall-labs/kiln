async function* generator() {
  try {
    yield "value";
  } finally {
    await Promise.resolve();
    console.log("generator-finally");
  }
}

async function main() {
  for await (const value of generator()) {
    console.log(value);
    break;
  }
  console.log("after-generator");

  const events = [];
  const iterable = {
    [Symbol.asyncIterator]() { return this; },
    next() { return Promise.resolve({ value: 2, done: false }); },
    return() {
      return Promise.resolve().then(() => {
        events.push("return");
        return { done: true };
      });
    },
  };
  for await (const value of iterable) {
    events.push("value" + value);
    break;
  }
  events.push("after");

  const original = new Error("body");
  const rejectsOnClose = {
    [Symbol.asyncIterator]() { return this; },
    next() { return Promise.resolve({ value: 1, done: false }); },
    return() { return Promise.reject(new Error("close")); },
  };
  let throwResult;
  try {
    for await (const _ of rejectsOnClose) throw original;
  } catch (error) {
    throwResult = error === original ? "body" : error.message;
  }

  const primitiveOnClose = {
    [Symbol.asyncIterator]() { return this; },
    next() { return Promise.resolve({ value: 1, done: false }); },
    return() { return Promise.resolve(1); },
  };
  let resultType;
  try {
    for await (const _ of primitiveOnClose) break;
  } catch (error) {
    resultType = error.constructor.name;
  }

  console.log(events.join(","), throwResult, resultType);
}

main().catch((error) => {
  console.log("unexpected", error && (error.stack || error.message));
});
