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

  const promisedValue = Promise.resolve("async delegate value");
  const asyncDelegate = {
    [Symbol.asyncIterator]() { return this; },
    async next() { return { value: promisedValue, done: false }; },
  };
  async function* asyncOuter() { yield* asyncDelegate; }
  const asyncResult = await asyncOuter().next();
  check(asyncResult.value === promisedValue,
    "real async delegate yields its raw value promise without awaiting it");

  let queuedCalls = 0;
  const queuedDelegate = {
    [Symbol.asyncIterator]() { return this; },
    async next() {
      queuedCalls++;
      return { value: `queued ${queuedCalls}`, done: false };
    },
  };
  async function* queuedOuter() { yield* queuedDelegate; }
  const queuedIterator = queuedOuter();
  const queuedFirst = queuedIterator.next();
  const queuedSecond = queuedIterator.next();
  const queuedResults = await Promise.all([queuedFirst, queuedSecond]);
  check(queuedResults[0].value === "queued 1" && queuedResults[1].value === "queued 2" &&
    queuedCalls === 2,
    "direct async-delegate values drain queued next() calls in order");

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
    [Symbol.asyncIterator]() { return this; },
    async next() {
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

  let caughtGetterError = false;
  let finallyRuns = 0;
  async function* catchingOuter() {
    try {
      yield* throwingDelegate;
    } catch (error) {
      caughtGetterError = error === getterError;
      yield "caught delegated getter";
    } finally {
      finallyRuns++;
    }
  }
  const catchingIterator = catchingOuter();
  const caughtResult = await catchingIterator.next();
  check(caughtResult.done === false && caughtResult.value === "caught delegated getter" &&
    caughtGetterError && finallyRuns === 0,
    "delegated value getter rejection resumes through the generator catch");
  const caughtCompletion = await catchingIterator.next();
  check(caughtCompletion.done === true && finallyRuns === 1,
    "delegated value getter catch preserves later finally completion");

  console.log("async yield* value semantics pass");
}

main().catch((error) => { throw error; });
