// kiln-native: stress=skip
// Abort when publication status is returned, before the actor module consumes the ticket.
import { MessagePort } from 'node:worker_threads';
import { isolate, spawnActor, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter', args: [0], mailboxCapacity: 2,
});
const beforePost = transfer(isolate(() => ({ answer: 17 })));
const earlyController = new AbortController();
const snapshotTrigger = { get value() { earlyController.abort(); return 1; } };
let earlyAborted = false;
try { await owner.ref.call('echo', [beforePost, snapshotTrigger], { signal: earlyController.signal }); }
catch (error) { earlyAborted = error.name === 'AbortError'; }
if (!earlyAborted) throw new Error('snapshot abort was not observed');
const retried = await owner.ref.call('echo', [beforePost]);
if (retried.answer !== 17) throw new Error('unpublished graph ticket was consumed by abort');

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
  // A completed reply is allowed to win a later abort. Hold the actor's result so this probe
  // exercises only publication followed by abort, regardless of how the worker is scheduled.
  try { await owner.ref.call('holdGraph', [ticket], { signal: controller.signal }); }
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
  console.log('unpublished graph retried; published graph stayed consumed after abort');
} finally {
  await owner.stop({ force: true });
}
