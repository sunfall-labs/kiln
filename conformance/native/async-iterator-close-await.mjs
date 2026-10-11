async function* generator() {
  try {
    yield "value";
  } finally {
    await Promise.resolve();
    console.log("generator-finally");
  }
}

async function main() {
  // A transpiled async-iterator helper can use a sync generator that yields from `finally` while
  // implementing its `return()`. The next internal generator step must finish the already-active
  // return completion instead of re-entering the generator body.
  function* returnThroughYieldingFinally() {
    try {
      yield "body";
    } finally {
      yield "cleanup";
    }
  }
  const inner = returnThroughYieldingFinally();
  inner.next();
  const closing = inner.return("closed");
  const closed = inner.next("resume");
  console.log(`yielding-finally ${closing.value}/${closing.done} ${closed.value}/${closed.done}`);

  function* returnOverriddenByFinally() {
    try {
      yield "body";
    } finally {
      yield "cleanup";
      return "override";
    }
  }
  const overridden = returnOverriddenByFinally();
  overridden.next();
  const overrideCleanup = overridden.return("original");
  const overrideResult = overridden.next("resume");
  console.log(`finally-return ${overrideCleanup.value}/${overrideCleanup.done} ${overrideResult.value}/${overrideResult.done}`);

  function* returnInterruptedByThrow() {
    try {
      yield "body";
    } finally {
      yield "cleanup";
      yield "cleanup-after-throw";
    }
  }
  const interrupted = returnInterruptedByThrow();
  interrupted.next();
  const interruptCleanup = interrupted.return("original");
  let interruptedWith;
  const injected = new Error("injected");
  try {
    interrupted.throw(injected);
  } catch (error) {
    interruptedWith = error === injected ? "same-throw" : "different-throw";
  }
  console.log(`finally-throw ${interruptCleanup.value}/${interruptCleanup.done} ${interruptedWith}`);

  function* returnSurvivesCaughtThrow() {
    try {
      yield "body";
    } finally {
      try {
        yield "cleanup";
        yield "pause";
      } catch (error) {
        yield "caught:" + error.message;
      }
    }
  }
  const caught = returnSurvivesCaughtThrow();
  caught.next();
  const caughtCleanup = caught.return("original");
  const caughtThrow = caught.throw(new Error("injected"));
  const caughtReturn = caught.next();
  console.log(`finally-caught-throw ${caughtCleanup.value}/${caughtCleanup.done} ${caughtThrow.value}/${caughtThrow.done} ${caughtReturn.value}/${caughtReturn.done}`);

  function* returnCompletesAfterCaughtThrow() {
    try {
      yield "body";
    } finally {
      try {
        yield "cleanup";
      } catch (_) {
        // Catching the injected throw lets the suspended return completion continue.
      }
    }
  }
  const caughtToCompletion = returnCompletesAfterCaughtThrow();
  caughtToCompletion.next();
  const caughtToCompletionCleanup = caughtToCompletion.return("original");
  const caughtToCompletionResult = caughtToCompletion.throw(new Error("injected"));
  console.log(`finally-caught-throw-complete ${caughtToCompletionCleanup.value}/${caughtToCompletionCleanup.done} ${caughtToCompletionResult.value}/${caughtToCompletionResult.done}`);

  function* returnKeepsObjectAcrossFinally() {
    try {
      yield "body";
    } finally {
      yield "cleanup";
      // Allocate after the caller has dropped its only reference to the return payload.
      for (let i = 0; i < 64; i++) ({ value: [i, { nested: true }] });
    }
  }
  const rooted = returnKeepsObjectAcrossFinally();
  rooted.next();
  const rootedCleanup = rooted.return({ marker: "held" });
  const rootedResult = rooted.next();
  console.log(`finally-return-root ${rootedCleanup.value}/${rootedCleanup.done} ${rootedResult.value.marker}/${rootedResult.done}`);

  function* returnReplacedByReturn() {
    try {
      yield "body";
    } finally {
      yield "cleanup";
      yield "cleanup-again";
    }
  }
  const replaced = returnReplacedByReturn();
  replaced.next();
  const replaceCleanup = replaced.return("original");
  const replaceResult = replaced.return("replacement");
  console.log(`finally-rereturn ${replaceCleanup.value}/${replaceCleanup.done} ${replaceResult.value}/${replaceResult.done}`);

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
