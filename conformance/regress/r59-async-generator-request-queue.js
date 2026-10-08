function check(condition, message) {
  if (!condition) throw new Error(message);
}

function barrier() {
  let release;
  const promise = new Promise(resolve => { release = resolve; });
  return { promise, release: () => release() };
}

async function queuedAcrossAwait() {
  const gate = barrier();
  async function* values() {
    await gate.promise;
    const sent = yield "first";
    yield sent;
  }
  const iterator = values();
  const first = iterator.next();
  let payload = { nested: { marker: "queued input survives" } };
  const second = iterator.next(payload);
  payload = null;
  let allocationChurn = [];
  for (let index = 0; index < 4096; index++) {
    allocationChurn.push({ index, nested: { index } });
  }
  allocationChurn = [];
  gate.release();
  const results = await Promise.all([first, second]);
  check(results[0].value === "first" && results[0].done === false,
    "the current async-generator request owns the awaited step");
  check(results[1].value.nested.marker === "queued input survives" && results[1].done === false,
    "a queued next resumes in order and retains its sent value across allocations");
}

async function queuedAcrossYieldStar() {
  let release;
  let calls = 0;
  const firstResult = new Promise(resolve => { release = resolve; });
  const delegate = {
    [Symbol.asyncIterator]() { return this; },
    next() {
      calls++;
      return calls === 1
        ? firstResult
        : Promise.resolve({ value: `delegate ${calls}`, done: false });
    },
  };
  async function* values() { yield* delegate; }
  const iterator = values();
  const first = iterator.next();
  const second = iterator.next();
  release({ value: "delegate 1", done: false });
  const results = await Promise.all([first, second]);
  check(results[0].value === "delegate 1" && results[1].value === "delegate 2" && calls === 2,
    "queued requests advance an async yield* delegate in order");
}

async function queuedAbruptRequests() {
  const returnGate = barrier();
  let finalized = false;
  async function* returning() {
    try {
      await returnGate.promise;
      yield "before return";
      yield "must not run";
    } finally {
      finalized = true;
    }
  }
  const returnIterator = returning();
  const firstReturnStep = returnIterator.next();
  const queuedReturn = returnIterator.return("returned");
  returnGate.release();
  const returned = await Promise.all([firstReturnStep, queuedReturn]);
  check(returned[0].value === "before return" && returned[0].done === false,
    "a queued return does not replace the active next request");
  check(returned[1].value === "returned" && returned[1].done === true && finalized,
    "a queued return unwinds after the active request settles");

  const throwGate = barrier();
  async function* throwing() {
    try {
      await throwGate.promise;
      yield "before throw";
    } catch (error) {
      yield `caught ${error}`;
    }
  }
  const throwIterator = throwing();
  const firstThrowStep = throwIterator.next();
  const queuedThrow = throwIterator.throw("queued error");
  throwGate.release();
  const thrown = await Promise.all([firstThrowStep, queuedThrow]);
  check(thrown[0].value === "before throw" && thrown[1].value === "caught queued error",
    "a queued throw is delivered after the active request yields");
}

async function main() {
  await queuedAcrossAwait();
  await queuedAcrossYieldStar();
  await queuedAbruptRequests();
  console.log("async-generator request queue passes");
}

main().catch(error => { throw error; });
