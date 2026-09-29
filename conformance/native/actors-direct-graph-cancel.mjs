// kiln-native: stress=skip
// Abort when publication status is returned, before the actor module consumes the ticket.
import { MessagePort } from 'node:worker_threads';
import { isolate, spawnActor, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter', args: [0], mailboxCapacity: 2,
});
const ticket = transfer(isolate(() => ({ answer: 42, values: [1, 2, 3] })));
const controller = new AbortController();
const internals = MessagePort[Symbol.for('kiln.messagePort')];
const original = internals.workerOp;
let published = false;
let aborted = false;
try {
  internals.workerOp = function (op, ...args) {
    const result = original.call(this, op, ...args);
    if (op === 31 && result === 4) {
      published = true;
      controller.abort();
    }
    return result;
  };
  try { await owner.ref.call('echo', [ticket], { signal: controller.signal }); }
  catch (error) { aborted = error.name === 'AbortError'; }
} finally {
  internals.workerOp = original;
}
try {
  if (!published || !aborted) throw new Error('publication-boundary abort was not observed');
  let consumed = false;
  try { await owner.ref.call('echo', [ticket]); }
  catch (error) { consumed = error.name === 'DataCloneError'; }
  if (!consumed) throw new Error('published graph ticket was restored after abort');
  console.log('published graph stayed consumed after abort');
} finally {
  await owner.stop();
}
