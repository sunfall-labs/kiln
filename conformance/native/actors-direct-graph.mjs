// kiln-native: stress=skip
// Every-allocation collection invalidates native candidate provenance; the native-only direct
// rollback probe checks that refusal and ticket restoration under stress.
import { isolate, spawnActor, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter', args: [0], mailboxCapacity: 2,
});
try {
  const ticket = transfer(isolate(() => ({ answer: 42, values: [1, 2, 3] })));
  const received = await owner.ref.call('echo', [ticket]);
  if (received.answer !== 42 || received.values.join(',') !== '1,2,3') {
    throw new Error('direct graph changed');
  }
  let reused = false;
  try { await owner.ref.call('echo', [ticket]); }
  catch (error) { reused = error.name === 'DataCloneError'; }
  if (!reused) throw new Error('consumed ticket reused');

  const repeated = transfer(isolate(() => {
    const child = { value: 9 };
    const graph = [child, child];
    graph.push(graph);
    return graph;
  }));
  const repeatedReply = await owner.ref.call('echo', [repeated]);
  if (repeatedReply[0] !== repeatedReply[1] || repeatedReply[2] !== repeatedReply ||
      repeatedReply[0].value !== 9) throw new Error('graph identity changed');

  const sparse = transfer(isolate(() => {
    const array = [];
    array.length = 4;
    array[3] = 17;
    return array;
  }));
  let sparseReply;
  try { sparseReply = await owner.ref.call('echo', [sparse]); }
  catch (error) { throw new Error(`sparse transfer: ${error.message}`); }
  if (sparseReply.length !== 4 || 0 in sparseReply || 1 in sparseReply || 2 in sparseReply ||
      sparseReply[3] !== 17) {
    throw new Error('sparse graph changed');
  }
  const numeric = transfer(isolate(() => {
    const value = { label: 7 };
    value[127] = 13;
    value[0] = 11;
    return value;
  }));
  const numericReply = await owner.ref.call('inspectNumericAfterAlloc', [numeric]);
  if (numericReply.join(',') !== '0,127,label,11,13,7,4096') {
    throw new Error('numeric-key graph changed');
  }

  const strings = transfer(isolate(() => ({
    latin1: 'caf\u00e9', wide: '\u03bb\ud83d\ude80', empty: '',
    values: ['first', 'second'],
  })));
  const stringReply = await owner.ref.call('echo', [strings]);
  if (stringReply.latin1 !== 'caf\u00e9' || stringReply.wide !== '\u03bb\ud83d\ude80' ||
      stringReply.empty !== '' || stringReply.values.join(',') !== 'first,second') {
    throw new Error('literal string graph changed');
  }

  let retained;
  const aliased = transfer(isolate(() => { retained = { answer: 17 }; return retained; }));
  let refused = false;
  try { await owner.ref.call('echo', [aliased]); }
  catch (error) { refused = error.name === 'DataCloneError'; }
  if (!refused || retained.answer !== 17) throw new Error('retained alias moved');
  retained = undefined;
  let retried;
  try { retried = await owner.ref.call('echo', [aliased]); }
  catch (error) { throw new Error(`refused-ticket retry: ${error.message}`); }
  if (retried.answer !== 17) throw new Error('refused ticket could not be retried');

  // Move an alias into a new outside holder after the factory has finished, then release
  // the original escape. A factory-only write log would miss the surviving holder edge.
  let lateAlias;
  const lateTicket = transfer(isolate(() => { lateAlias = { answer: 23 }; return lateAlias; }));
  const outsideHolder = { value: lateAlias };
  lateAlias = undefined;
  let lateRefused = false;
  try { await owner.ref.call('echo', [lateTicket]); }
  catch (error) { lateRefused = error.name === 'DataCloneError'; }
  if (!lateRefused || outsideHolder.value.answer !== 23) {
    throw new Error('post-factory alias moved');
  }
  outsideHolder.value = undefined;
  const lateRetry = await owner.ref.call('echo', [lateTicket]);
  if (lateRetry.answer !== 23) throw new Error('post-factory alias retry failed');

  // Warm the decoupled module's cached private-field setter with ordinary object values.
  // A later candidate write must record the outside owner even on the cache hit.
  class PrivateHolder {
    #held;
    set(value) { this.#held = value; }
    read() { return this.#held; }
  }
  const privateHolder = new PrivateHolder();
  privateHolder.set({ answer: 0 });
  privateHolder.set({ answer: -1 });
  let privateAlias;
  const privateTicket = transfer(isolate(() => {
    privateAlias = { answer: 29 };
    return privateAlias;
  }));
  privateHolder.set(privateAlias);
  privateAlias = undefined;
  let privateRefused = false;
  try { await owner.ref.call('echo', [privateTicket]); }
  catch (error) { privateRefused = error.name === 'DataCloneError'; }
  if (!privateRefused || privateHolder.read().answer !== 29) {
    throw new Error('cached private-field alias moved');
  }
  privateHolder.set(undefined);
  const privateRetry = await owner.ref.call('echo', [privateTicket]);
  if (privateRetry.answer !== 29) throw new Error('private-field alias retry failed');

  let retainedChild;
  const childAlias = transfer(isolate(() => ({ child: retainedChild = { value: 21 } })));
  let childRefused = false;
  try { await owner.ref.call('echo', [childAlias]); }
  catch (error) { childRefused = error.name === 'DataCloneError'; }
  if (!childRefused || retainedChild.value !== 21) throw new Error('retained child alias moved');

  let weak;
  const weakAlias = transfer(isolate(() => {
    const value = { answer: 31 };
    weak = new WeakRef(value);
    return value;
  }));
  let weakRefused = false;
  try { await owner.ref.call('echo', [weakAlias]); }
  catch (error) { weakRefused = error.name === 'DataCloneError'; }
  if (!weakRefused || weak.deref()?.answer !== 31) {
    throw new Error('sender weak alias moved');
  }
  console.log('direct graphs moved; retained aliases refused');
} finally {
  await owner.stop();
}
