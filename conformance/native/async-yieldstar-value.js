function check(condition, message) {
  if (!condition) throw new Error(message);
}

async function main() {
  const nested = { payload: { answer: 42 } };
  let syncCalls = 0;
  const syncDelegate = {
    [Symbol.iterator]() { return this; },
    next() {
      syncCalls++;
      if (syncCalls === 1) return { value: Promise.resolve(nested), done: false };
      return { value: "sync completion", done: true };
    },
  };
  async function* syncOuter() {
    const completion = yield* syncDelegate;
    yield completion;
  }
  const syncIterator = syncOuter();
  const syncFirst = await syncIterator.next();
  check(syncFirst.done === false && syncFirst.value === nested,
    "async yield* awaits and exposes the delegated value, not its iterator result");
  const syncSecond = await syncIterator.next();
  check(syncSecond.done === false && syncSecond.value === "sync completion",
    "sync delegate completion becomes the yield* expression result");

  let valueReads = 0;
  const getterDelegate = {
    [Symbol.iterator]() { return this; },
    next() {
      return {
        done: false,
        get value() {
          valueReads++;
          return nested;
        },
      };
    },
  };
  async function* getterOuter() { yield* getterDelegate; }
  const getterResult = await getterOuter().next();
  check(getterResult.value === nested && valueReads === 1,
    "delegated iterator-result value getter is read exactly once");

  const getterError = new Error("delegated value getter");
  const throwingDelegate = {
    [Symbol.iterator]() { return this; },
    next() {
      return {
        done: false,
        get value() { throw getterError; },
      };
    },
  };
  async function* throwingOuter() { yield* throwingDelegate; }
  let rejected = false;
  try {
    await throwingOuter().next();
  } catch (error) {
    rejected = error === getterError;
  }
  check(rejected, "delegated value getter failure rejects next() with the original error");

  console.log("async yield* value semantics pass");
}

main().catch((error) => { throw error; });
