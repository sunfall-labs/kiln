function assertEvents(actual, expected, label) {
  if (actual.join('|') !== expected.join('|')) {
    throw new Error(`${label}: expected ${expected.join('|')}, got ${actual.join('|')}`);
  }
}

function assertStep(actual, value, done, label) {
  if (!actual || actual.value !== value || actual.done !== done) {
    throw new Error(`${label}: expected { value: ${String(value)}, done: ${done} }, got ${String(actual?.value)}/${String(actual?.done)}`);
  }
}

async function closedGenerator() {
  const generator = (async function* () {})();
  await generator.next();
  return generator;
}

function installThenGetter(onOuter, events) {
  const prior = Object.getOwnPropertyDescriptor(Object.prototype, 'then');
  let nesting = 0;
  let triggered = false;
  Object.defineProperty(Object.prototype, 'then', {
    configurable: true,
    enumerable: false,
    get() {
      if (nesting === 0 && !triggered) {
        triggered = true;
        events.push('outer-enter');
        nesting++;
        onOuter();
        nesting--;
        events.push('outer-exit');
      } else {
        events.push('nested-enter');
        events.push('nested-exit');
      }
      return undefined;
    },
  });
  return () => {
    if (prior) Object.defineProperty(Object.prototype, 'then', prior);
    else delete Object.prototype.then;
  };
}

async function checkClosedNextReentry() {
  const generator = await closedGenerator();
  const events = [];
  let nested;
  const restore = installThenGetter(() => { nested = generator.next(); }, events);
  let result;
  try {
    result = await generator.next();
    await nested;
  } finally {
    restore();
  }
  assertEvents(events, ['outer-enter', 'nested-enter', 'nested-exit', 'outer-exit'], 'closed next reentry');
  assertStep(result, undefined, true, 'closed next');
}

async function checkClosedReturnReentry() {
  const generator = await closedGenerator();
  const events = [];
  let nested;
  const restore = installThenGetter(() => { nested = generator.next(); }, events);
  let result;
  try {
    result = await generator.return(37);
    await nested;
  } finally {
    restore();
  }
  assertEvents(events, ['outer-enter', 'nested-enter', 'nested-exit', 'outer-exit'], 'closed return reentry');
  assertStep(result, 37, true, 'closed return');
}


async function checkClosedReturnThenableReentry() {
  const generator = await closedGenerator();
  const events = [];
  let nested;
  const operand = {
    get then() {
      events.push('return-operand-then');
      return resolve => resolve(38);
    },
  };
  const restore = installThenGetter(() => { nested = generator.next(); }, events);
  let result;
  try {
    result = await generator.return(operand);
    await nested;
  } finally {
    restore();
  }
  assertEvents(events, [
    'return-operand-then', 'outer-enter', 'nested-enter', 'nested-exit', 'outer-exit',
  ], 'closed return thenable reentry');
  assertStep(result, 38, true, 'closed return thenable');
}

async function checkClosedReturnThenableAndOlderQueuedRequest() {
  const generator = await closedGenerator();
  const events = [];
  let resolveOperand;
  const operand = new Promise(resolve => { resolveOperand = resolve; });
  const outer = generator.return(operand);
  const completionOrder = [];
  const older = generator.next();
  older.then(() => { completionOrder.push('older'); });
  let nested;
  const restore = installThenGetter(() => {
    nested = generator.next();
    nested.then(() => { completionOrder.push('nested'); });
  }, events);
  resolveOperand(9);
  let outerResult;
  let olderResult;
  let nestedResult;
  try {
    outerResult = await outer;
    olderResult = await older;
    nestedResult = await nested;
  } finally {
    restore();
  }
  assertEvents(events, [
    'outer-enter',
    'nested-enter', 'nested-exit',
    'nested-enter', 'nested-exit',
    'outer-exit',
  ], 'older queued request FIFO handoff');
  assertStep(outerResult, 9, true, 'closed awaited return');
  assertStep(olderResult, undefined, true, 'older queued request');
  assertStep(nestedResult, undefined, true, 'new reentrant request');
  assertEvents(completionOrder, ['older', 'nested'], 'older request must complete before reentrant request');
}

async function checkReentrantReturnKeepsTheLane() {
  const generator = await closedGenerator();
  const events = [];
  let resolveOperand;
  const operand = {
    get then() {
      events.push('return-operand-then');
      return resolve => { resolveOperand = resolve; };
    },
  };
  let returnPromise;
  let queuedPromise;
  const restore = installThenGetter(() => {
    returnPromise = generator.return(operand);
    queuedPromise = generator.next();
  }, events);
  let outerResult;
  let returnResult;
  let queuedResult;
  let queuedSettled = false;
  try {
    const outerPromise = generator.next();
    queuedPromise?.then(() => { queuedSettled = true; });
    outerResult = await outerPromise;
    await Promise.resolve();
    if (queuedSettled) throw new Error('next queued behind pending return settled too early');
    if (typeof resolveOperand !== 'function')
      throw new Error('reentrant return did not start awaiting its operand');
    resolveOperand(55);
    returnResult = await returnPromise;
    queuedResult = await queuedPromise;
  } finally {
    restore();
  }
  assertEvents(events, [
    'outer-enter', 'return-operand-then', 'outer-exit',
    'nested-enter', 'nested-exit',
    'nested-enter', 'nested-exit',
  ], 'reentrant pending return lane');
  assertStep(outerResult, undefined, true, 'outer closed next');
  assertStep(returnResult, 55, true, 'reentrant closed return');
  assertStep(queuedResult, undefined, true, 'request behind pending return');
}

await checkClosedNextReentry();
await checkClosedReturnReentry();
await checkClosedReturnThenableReentry();
await checkClosedReturnThenableAndOlderQueuedRequest();
await checkReentrantReturnKeepsTheLane();
console.log('async-generator-closed-reentry: pass');
