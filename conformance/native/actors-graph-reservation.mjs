import { spawnActor } from 'kiln:actors';
import { MessageChannel, MessagePort } from 'node:worker_threads';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter',
  args: [1],
});
const internals = MessagePort[Symbol.for('kiln.messagePort')];
const endpoint = internals.workerOp(23, owner.ref.address);
const sender = internals.internalPort(endpoint, null, null, false);
const id = internals.workerOp(25);
const reply = new MessageChannel();
const response = new Promise(resolve => {
  reply.port1.on('message', resolve);
  reply.port1.start();
});
sender.postMessage({ kind: 'reserve', id, reply: reply.port2 }, [reply.port2]);
const grant = await response;
if (grant?.kind !== 'grant') throw new Error('actor did not grant capacity');
const refusal = new Promise(resolve => reply.port1.once('message', resolve));
sender.postMessage({
  kind: 'invoke', id, method: 'echo',
  args: [{ __kilnActorGraphReservation: (grant.graphReservation || 0) + 1 }],
  graphReservation: grant.graphReservation || 0,
});
const answer = await refusal;
if (answer?.kind !== 'cloneError') throw new Error('forged graph placeholder was accepted');
if (await owner.ref.call('echo', [17]) !== 17) throw new Error('actor did not recover after refusal');
await owner.stop();
console.log('graph reservation refused safely');
