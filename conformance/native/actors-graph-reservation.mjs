// native-env: KILN_ENGINE_PRIVATE_FOR_TESTS=1
// Drives the actor protocol through the engine-private channel, which a program never sees:
// the switch above (a developer switch, off by default in both engines) leaves it readable.
import { spawnActor } from 'kiln:actors';
import { MessageChannel } from 'node:worker_threads';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter',
  args: [1],
});
const internals = globalThis.__kilnPrivate.messagePort;
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
// A correctly shaped placeholder is held until the native handoff resolves. Cancelling that
// request must free the actor's capacity and its one-use receiver reservation.
const heldId = internals.workerOp(25);
const heldReply = new MessageChannel();
const heldGrant = new Promise(resolve => {
  heldReply.port1.once('message', resolve);
  heldReply.port1.start();
});
sender.postMessage({ kind: 'reserve', id: heldId, reply: heldReply.port2 }, [heldReply.port2]);
const held = await heldGrant;
if (held?.kind !== 'grant') throw new Error('held actor call was not granted');
if (held.graphReservation) {
  sender.postMessage({
    kind: 'invoke', id: heldId, method: 'echo',
    args: [{ __kilnActorGraphReservation: held.graphReservation }],
    graphReservation: held.graphReservation,
  });
}
sender.postMessage({ kind: 'cancel', id: heldId });
if (await owner.ref.call('echo', [17]) !== 17) throw new Error('actor did not recover after refusal');
await owner.stop();
console.log('graph reservation refused safely');
