// Test specification compliance for async generator delegation protocol edges:
// return, throw, unwrapping differences between sync and async iterators, and iterator closing.

async function testAsyncReturn() {
  const log = [];
  const delegate = {
    [Symbol.asyncIterator]() {
      let returnCount = 0;
      return {
        next() { return Promise.resolve({ value: 'v1', done: false }); },
        return(arg) {
          log.push(`return:${arg}`);
          returnCount++;
          if (returnCount === 1) {
            return Promise.resolve({ value: 'ret-val-1', done: false });
          }
          return Promise.resolve({ value: 'ret-val-2', done: true });
        }
      };
    }
  };
  async function* g() {
    yield* delegate;
    return 'unreachable';
  }
  const it = g();
  const step1 = await it.next();
  const ret1 = await it.return('arg1');
  const ret2 = await it.return('arg2');
  console.log(`async-return: step1=${step1.value},${step1.done} ret1=${ret1.value},${ret1.done} ret2=${ret2.value},${ret2.done} log=${log.join(',')}`);
}

async function testAsyncThrow() {
  const log = [];
  const delegate = {
    [Symbol.asyncIterator]() {
      let throwCount = 0;
      return {
        next() { return Promise.resolve({ value: 'v1', done: false }); },
        throw(err) {
          log.push(`throw:${err}`);
          throwCount++;
          if (throwCount === 1) {
            return Promise.resolve({ value: 'throw-val-1', done: false });
          }
          return Promise.resolve({ value: 'throw-val-2', done: true });
        }
      };
    }
  };
  async function* g() {
    const res = yield* delegate;
    log.push(`after:${res}`);
    return 'fin';
  }
  const it = g();
  const step1 = await it.next();
  const thr1 = await it.throw('err1');
  const thr2 = await it.throw('err2');
  console.log(`async-throw: step1=${step1.value},${step1.done} thr1=${thr1.value},${thr1.done} thr2=${thr2.value},${thr2.done} log=${log.join(',')}`);
}

async function testSyncReturnThenable() {
  const log = [];
  const delegate = {
    [Symbol.iterator]() {
      let returnCount = 0;
      return {
        next() { return { value: 's1', done: false }; },
        return(arg) {
          log.push(`sync-return:${arg}`);
          returnCount++;
          return {
            value: Promise.resolve(`sync-ret-${returnCount}`),
            done: returnCount > 1
          };
        }
      };
    }
  };
  async function* g() {
    yield* delegate;
  }
  const it = g();
  await it.next();
  const r1 = await it.return('a1');
  const r2 = await it.return('a2');
  console.log(`sync-return: r1=${r1.value},${r1.done} r2=${r2.value},${r2.done} log=${log.join(',')}`);
}

async function testPromiseNotUnwrapped() {
  const innerPromise = Promise.resolve('not-unwrapped');
  const delegate = {
    [Symbol.asyncIterator]() { return this; },
    next() { return { done: false, value: innerPromise }; }
  };
  async function* g() {
    yield* delegate;
  }
  const it = g();
  const step = await it.next();
  console.log(`promise-not-unwrapped: is-same=${step.value === innerPromise}`);
}

async function testReturnMissingValueAwaited() {
  const delegate = {
    [Symbol.asyncIterator]() { return this; },
    next() { return { value: 1, done: false }; }
  };
  async function* g() {
    yield* delegate;
  }
  const it = g();
  await it.next();
  const ret = await it.return(Promise.resolve(42));
  console.log(`return-missing-awaited: val=${ret.value} done=${ret.done}`);
}

async function testThrowMissingClosesIterator() {
  let closed = false;
  const delegate = {
    [Symbol.iterator]() {
      return {
        next() { return { value: 1, done: false }; },
        return() { closed = true; return { done: true }; }
      };
    }
  };
  async function* g() {
    yield* delegate;
  }
  const it = g();
  await it.next();
  let caughtError = null;
  try {
    await it.throw(new Error('no throw'));
  } catch (err) {
    caughtError = err;
  }
  console.log(`throw-missing-closed: closed=${closed} is-type-error=${caughtError instanceof TypeError}`);
}

async function main() {
  await testAsyncReturn();
  await testAsyncThrow();
  await testSyncReturnThenable();
  await testPromiseNotUnwrapped();
  await testReturnMissingValueAwaited();
  await testThrowMissingClosesIterator();
}

main().catch(err => {
  console.error('FAILED', err);
  process.exit(1);
});
