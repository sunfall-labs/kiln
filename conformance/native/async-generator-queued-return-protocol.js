// Async-generator requests stay FIFO across result resolution, Await, delegation and finally.
async function main() {
  const output = [];

  async function* returnsThroughFinally() {
    try {
      yield "ready";
    } finally {
      yield "closing";
    }
  }
  const returning = returnsThroughFinally();
  await returning.next();
  const pendingReturn = returning.return(Promise.resolve("return-value"));
  const closing = await pendingReturn;
  const returned = await returning.next();
  output.push(`return-finally:${closing.value}/${closing.done}:${returned.value}/${returned.done}`);

  const rejection = new Error("return-operand-rejected");
  async function* rejectedReturn() {
    yield "first";
    yield "second";
  }
  const rejecting = rejectedReturn();
  await rejecting.next();
  try {
    await rejecting.return(Promise.reject(rejection));
  } catch (error) {
    output.push(`return-reject:${error.message}`);
  }
  const afterRejectedReturn = await rejecting.next();
  output.push(`after-reject:${afterRejectedReturn.value}/${afterRejectedReturn.done}`);

  async function* overriddenReturn() {
    try {
      yield "first";
    } finally {
      yield "finally-yield";
      return "finally-return";
    }
  }
  const overriding = overriddenReturn();
  await overriding.next();
  const overrideYield = await overriding.return("requested-return");
  const overrideDone = await overriding.next();
  output.push(`override:${overrideYield.value}/${overrideYield.done}:${overrideDone.value}/${overrideDone.done}`);

  const rejectedSyncValue = new Error("wrapped-done-value-rejected");
  async function* wrappedDoneValue() {
    yield* {
      [Symbol.iterator]() {
        return { next() { return { value: Promise.reject(rejectedSyncValue), done: true }; } };
      },
    };
  }
  try {
    await wrappedDoneValue().next();
  } catch (error) {
    output.push(`wrapped-done-reject:${error.message}`);
  }

  let nextIndex = 0;
  const asyncDelegate = {
    [Symbol.asyncIterator]() {
      return { next() { return { value: nextIndex++, done: false }; } };
    },
  };
  async function* delegated() {
    yield* asyncDelegate;
  }
  const iterator = delegated();
  const requests = [iterator.next()];
  for (let index = 1; index < 4096; index += 1) requests.push(iterator.next());
  const values = await Promise.all(requests);
  const done = await iterator.return("done-value");
  output.push(`queue:${values.length}/${values[0].value}/${values[4095].value}/${values[4095].done}:${done.value}/${done.done}`);

  async function reentrantGetter(withOlderRequest) {
    const events = [];
    let reentered = false;
    let reentrant;
    let current;
    const oldThen = Object.getOwnPropertyDescriptor(Object.prototype, "then");
    Object.defineProperty(Object.prototype, "then", {
      configurable: true,
      get() {
        const value = Object.getOwnPropertyDescriptor(this, "value");
        const doneProperty = Object.getOwnPropertyDescriptor(this, "done");
        if (!reentered && value?.value === "first" && doneProperty?.value === false) {
          reentered = true;
          events.push("getter:start");
          reentrant = current.next().then((result) => {
            events.push(`reentrant:${result.value ?? "done"}`);
            return result;
          });
          events.push("getter:end");
        }
        return undefined;
      },
    });
    try {
      async function* sequence() {
        events.push("resume:first");
        yield "first";
        events.push("resume:second");
        yield "second";
        events.push("resume:third");
        yield "third";
      }
      current = sequence();
      const first = current.next().then((result) => {
        events.push(`first:${result.value}`);
        return result;
      });
      let second;
      if (withOlderRequest) {
        second = current.next().then((result) => {
          events.push(`second:${result.value}`);
          return result;
        });
      }
      await Promise.all([first, ...(second ? [second] : []), new Promise((resolve) => {
        const poll = () => (reentrant ? resolve(reentrant) : queueMicrotask(poll));
        poll();
      })]);
      return events.join(",");
    } finally {
      if (oldThen) Object.defineProperty(Object.prototype, "then", oldThen);
      else delete Object.prototype.then;
    }
  }
  output.push(`reentrant-empty:${await reentrantGetter(false)}`);
  output.push(`reentrant-queued:${await reentrantGetter(true)}`);

  console.log(output.join("\n"));
}

main();
