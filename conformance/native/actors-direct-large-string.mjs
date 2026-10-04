// Collection settles dead candidate blocks and preserves the live graph's provenance.
import { isolate, spawnActor, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter', args: [0], mailboxCapacity: 2,
});
try {
  const ticket = transfer(isolate(() => ({ text: 'q'.repeat(300000), answer: 42 })));
  const result = await owner.ref.call('echo', [ticket]);
  if (result.answer !== 42 || result.text.length !== 300000 ||
      result.text[0] !== 'q' || result.text[299999] !== 'q') {
    throw new Error('large graph string changed');
  }
  let retained;
  const aliased = transfer(isolate(() => ({ child: retained = { text: 'z'.repeat(300000) } })));
  let refused = false;
  try { await owner.ref.call('echo', [aliased]); }
  catch (error) { refused = error.name === 'DataCloneError'; }
  if (!refused || retained.text.length !== 300000 || retained.text[299999] !== 'z') {
    throw new Error('large string alias moved');
  }
  retained = undefined;
  const retried = await owner.ref.call('echo', [aliased]);
  if (retried.child.text.length !== 300000 || retried.child.text[299999] !== 'z') {
    throw new Error('large string retry changed');
  }
  console.log('large graph strings moved; retained alias refused and retried');
} finally {
  await owner.stop();
}
