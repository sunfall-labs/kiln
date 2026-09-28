import { spawnActor, connectActor, isolate, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter',
  args: [4],
  mailboxCapacity: 1,
});
console.log(await owner.ref.call('increment', [3]));
const address = structuredClone(owner.ref.address);
const second = connectActor(address);
console.log(await second.call('increment', [5]));
try { await second.call('fail', []); }
catch (error) { console.log(error.name, error.remoteName, error.message); }
const data = new ArrayBuffer(4);
new Uint8Array(data)[0] = 23;
const bufferTicket = transfer(data);
const ticketState = Reflect.ownKeys(bufferTicket)
  .map(key => Object.getOwnPropertyDescriptor(bufferTicket, key)?.value)
  .find(value => value && typeof value === 'object' && 'consumed' in value);
if (!ticketState) throw new Error('buffer ticket has no state');
try { ticketState.value = new ArrayBuffer(4); } catch {}
if (ticketState.value !== data) throw new Error('buffer ticket payload was replaced');
const result = await owner.ref.call('echo', [bufferTicket]);
if (!ticketState.consumed) throw new Error('accepted buffer ticket was not consumed');
if (ticketState.value !== undefined) throw new Error('consumed ticket retained its ArrayBuffer');
try { ticketState.consumed = false; } catch {}
if (!ticketState.consumed) throw new Error('buffer ticket consumption was reset');
console.log(data.byteLength, new Uint8Array(result)[0]);
let retainedGraphAlias;
const isolated = isolate(() => {
  retainedGraphAlias = { answer: 42 };
  return retainedGraphAlias;
});
for (const key of Reflect.ownKeys(isolated)) {
  const value = Object.getOwnPropertyDescriptor(isolated, key)?.value;
  if (value?.answer === 42 || value?.value?.answer === 42) throw new Error('isolated graph leaked through handle');
}
const graphTicket = transfer(isolated);
const graphTicketState = Reflect.ownKeys(graphTicket)
  .map(key => Object.getOwnPropertyDescriptor(graphTicket, key)?.value)
  .find(value => value && typeof value === 'object' && 'consumed' in value);
if (!graphTicketState || graphTicketState.consumed) throw new Error('new graph ticket is unusable');
const forgedTicket = {};
Object.defineProperty(forgedTicket, Symbol.for('kiln.actor.ticket'), { value: graphTicketState });
let forgedRefused = false;
try { await owner.ref.call('echo', [forgedTicket]); }
catch (error) {
  forgedRefused = error.name === 'DataCloneError'
    && error.message === 'graph transfer needs ownership proof';
}
if (!forgedRefused || graphTicketState.consumed || graphTicketState.value !== isolated) {
  throw new Error('forged graph ticket moved or consumed the genuine isolated handle');
}
for (const key of Reflect.ownKeys(graphTicket)) {
  const value = Object.getOwnPropertyDescriptor(graphTicket, key)?.value;
  if (value?.answer === 42 || value?.value?.answer === 42) throw new Error('isolated graph leaked through ticket');
}
const actorInternals = MessagePort[Symbol.for('kiln.messagePort')];
const statusWorkerOp = actorInternals.workerOp;
let delayedGraphStatus = false;
try {
  // Native requests may spend a turn pending after posting the held placeholder. Keep the
  // ticket suspended across one extra poll, then require the terminal refusal to restore it.
  actorInternals.workerOp = (operation, ...args) => {
    if (operation === 31 && !delayedGraphStatus) {
      delayedGraphStatus = true;
      return 0;
    }
    return statusWorkerOp(operation, ...args);
  };
  for (let attempt = 0; attempt < 2; attempt++) {
    let refused = false;
    try { await owner.ref.call('echo', [graphTicket]); }
    catch (error) {
      refused = error.name === 'DataCloneError' && error.message === 'graph transfer needs ownership proof';
      if (attempt === 0) console.log(error.name, error.message);
    }
    if (!refused || graphTicketState.consumed || retainedGraphAlias.answer !== 42) {
      throw new Error('failed graph handoff consumed its ticket');
    }
  }
} finally {
  actorInternals.workerOp = statusWorkerOp;
}
let factoryFailed = false;
try { isolate(() => { throw new Error('factory failed'); }); }
catch (error) { factoryFailed = error.message === 'factory failed'; }
if (!factoryFailed) throw new Error('failed isolate factory did not unwind');
const senderEndpoint = actorInternals.workerOp(23, owner.ref.address);
if (actorInternals.workerOp(33, senderEndpoint) !== false) {
  throw new Error('a nonworker realm claimed the actor inbox');
}
if (await owner.ref.call('probeMailboxBind', [senderEndpoint]) !== false) {
  throw new Error('actor worker claimed a different endpoint');
}
const retainedHandle = graphTicketState.suspend();
if (graphTicketState.value !== undefined) throw new Error('suspended ticket retained its handle');
graphTicketState.restore(retainedHandle);
if (graphTicketState.value !== isolated) throw new Error('restored ticket changed identity');
const originalWorkerOp = actorInternals.workerOp;
const factoryError = new Error('original factory exception');
let preservedFactoryError = false;
try {
  actorInternals.workerOp = (operation, ...args) => {
    const result = originalWorkerOp(operation, ...args);
    if (operation === 28) return true;
    if (operation === 29) return false; // Report cleanup failure after actually closing the scope.
    return result;
  };
  try { isolate(() => { throw factoryError; }); }
  catch (error) { preservedFactoryError = error === factoryError; }
} finally {
  actorInternals.workerOp = originalWorkerOp;
}
if (!preservedFactoryError) throw new Error('candidate cleanup masked the factory exception');
const nested = isolate(() => isolate(() => ({ nested: true })));
const nestedTicket = transfer(nested);
let nestedRefused = false;
try { await owner.ref.call('echo', [nestedTicket]); }
catch (error) {
  if (error.name !== 'DataCloneError') throw error;
  nestedRefused = true;
}
if (!nestedRefused) throw new Error('nested isolated graph escaped');
await owner.stop();
try { await second.call('increment', [1]); }
catch (error) { console.log(error.name); }
