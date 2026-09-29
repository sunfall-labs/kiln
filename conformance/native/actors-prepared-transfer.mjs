import { spawnActor, prepareTransfer, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter',
  args: [1],
});

const input = [3, 5, 8];
const handle = prepareTransfer(input);
input[0] = 99;
const ticket = transfer(handle);
const answer = await owner.ref.call('echo', [ticket]);
if (!Array.isArray(answer) || answer.join(',') !== '3,5,8' || input[0] !== 99) {
  throw new Error('prepared array snapshot changed with its source');
}
let consumed = false;
try { await owner.ref.call('echo', [ticket]); }
catch (error) { consumed = error.name === 'DataCloneError'; }
if (!consumed) throw new Error('prepared graph ticket was reusable');

const object = { answer: 42, values: [3, 5, 8] };
const prepared = prepareTransfer(object);
object.answer = 7;
const inspected = await owner.ref.call('inspectGraph', [transfer(prepared)]);
if (inspected.join('|') !== 'true|42|3,5,8' || object.answer !== 7) {
  throw new Error('prepared object snapshot changed with its source');
}
let unsupported = false;
try { prepareTransfer([new Date(0)]); }
catch (error) { unsupported = error.name === 'DataCloneError'; }
if (!unsupported) throw new Error('unsupported prepared graph was accepted');
const locked = { value: 1 };
Object.defineProperty(locked, 'value', { writable: false });
let descriptorRefused = false;
try { prepareTransfer(locked); }
catch (error) { descriptorRefused = error.name === 'DataCloneError'; }
if (!descriptorRefused) throw new Error('non-default property descriptor was accepted');

const left = transfer(prepareTransfer([1]));
const right = transfer(prepareTransfer([2]));
let multiple = false;
try { await owner.ref.call('echo', [[left, right]]); }
catch (error) { multiple = error.name === 'DataCloneError'; }
if (!multiple || (await owner.ref.call('echo', [left]))[0] !== 1 ||
    (await owner.ref.call('echo', [right]))[0] !== 2) {
  throw new Error('a refused multi-graph call consumed a ticket');
}

const cycle = [];
cycle.push(cycle);
const movedCycle = await owner.ref.call('echo', [transfer(prepareTransfer(cycle))]);
if (movedCycle[0] !== movedCycle) throw new Error('prepared cycle lost identity');

await owner.stop();
console.log('prepared snapshots move once');
