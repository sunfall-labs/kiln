import { setFlagsFromString } from 'node:v8';
import { runInNewContext } from 'node:vm';

setFlagsFromString('--expose_gc');
const collect = runInNewContext('gc');
setFlagsFromString('--no-expose_gc');

const control = new WeakRef({});
let deadLocal;
let capturedLocal;
let returnedLocal;
let thrownLocal;
let asyncThrownLocal;
let escaped;
let finallyRan = false;
function* completed() {
  const dead = { marker: 'dead' };
  deadLocal = new WeakRef(dead);
  const captured = { marker: 'captured' };
  capturedLocal = new WeakRef(captured);
  escaped = () => captured.marker;
  yield 0;
  return 'completed';
}

function* completedByReturn() {
  const dead = { marker: 'return' };
  returnedLocal = new WeakRef(dead);
  try {
    yield 0;
  } finally {
    finallyRan = true;
  }
}
function* completedByThrow() {
  const dead = { marker: 'throw' };
  thrownLocal = new WeakRef(dead);
  yield 0;
}
async function* completedAsyncByThrow() {
  const dead = { marker: 'async-throw' };
  asyncThrownLocal = new WeakRef(dead);
  yield 0;
}

const iterator = completed();
iterator.next();
const result = iterator.next();
const returnIterator = completedByReturn();
returnIterator.next();
const returnValue = { marker: 'return-value' };
const returnResult = returnIterator.return(returnValue);
const throwIterator = completedByThrow();
throwIterator.next();
const thrown = new Error('terminal throw');
let caught;
try {
  throwIterator.throw(thrown);
} catch (error) {
  caught = error;
}
const asyncThrowIterator = completedAsyncByThrow();
await asyncThrowIterator.next();
let asyncCaught;
try {
  await asyncThrowIterator.throw(thrown);
} catch (error) {
  asyncCaught = error;
}

async function finish() {
  for (let i = 0; i < 8; i++) {
    await new Promise(resolve => setTimeout(resolve, 0));
    collect();
  }
  console.log(JSON.stringify({
    controlDead: control.deref() === undefined,
    deadLocalCollected: deadLocal.deref() === undefined,
    returnedLocalCollected: returnedLocal.deref() === undefined,
    thrownLocalCollected: thrownLocal.deref() === undefined,
    asyncThrownLocalCollected: asyncThrownLocal.deref() === undefined,
    finallyRan,
    caughtSame: caught === thrown,
    asyncCaughtSame: asyncCaught === thrown,
    returnDone: returnResult.done,
    returnValuePreserved: returnResult.value === returnValue,
    capturedLocalAlive: capturedLocal.deref() !== undefined,
    escapedValue: escaped(),
    done: iterator.next().done,
    returned: result.value
  }));
}

finish().catch(console.error);
