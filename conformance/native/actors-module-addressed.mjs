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
try { ticketState.consumed = false; } catch {}
if (!ticketState.consumed) throw new Error('buffer ticket consumption was reset');
console.log(data.byteLength, new Uint8Array(result)[0]);
const isolated = isolate(() => ({ answer: 42 }));
for (const key of Reflect.ownKeys(isolated)) {
  const value = Object.getOwnPropertyDescriptor(isolated, key)?.value;
  if (value?.answer === 42 || value?.value?.answer === 42) throw new Error('isolated graph leaked through handle');
}
const graphTicket = transfer(isolated);
const graphTicketState = Reflect.ownKeys(graphTicket)
  .map(key => Object.getOwnPropertyDescriptor(graphTicket, key)?.value)
  .find(value => value && typeof value === 'object' && 'consumed' in value);
if (!graphTicketState || graphTicketState.consumed) throw new Error('new graph ticket is unusable');
for (const key of Reflect.ownKeys(graphTicket)) {
  const value = Object.getOwnPropertyDescriptor(graphTicket, key)?.value;
  if (value?.answer === 42 || value?.value?.answer === 42) throw new Error('isolated graph leaked through ticket');
}
for (let attempt = 0; attempt < 2; attempt++) {
  let refused = false;
  try { await owner.ref.call('echo', [graphTicket]); }
  catch (error) {
    refused = error.name === 'DataCloneError' && error.message === 'graph transfer needs ownership proof';
    if (attempt === 0) console.log(error.name, error.message);
  }
  if (!refused || graphTicketState.consumed) {
    throw new Error('failed graph handoff consumed its ticket');
  }
}
await owner.stop();
try { await second.call('increment', [1]); }
catch (error) { console.log(error.name); }
