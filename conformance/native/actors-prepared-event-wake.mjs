import { isolate, prepareTransfer, spawnActor, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter',
  args: [1],
});

const timer = globalThis.setTimeout;
let timerCalls = 0;
globalThis.setTimeout = () => {
  timerCalls++;
  throw new Error('a prepared graph call polled with setTimeout');
};
try {
  for (let index = 0; index < 8; index++) {
    const source = { answer: index };
    const result = await owner.ref.call('echo', [transfer(prepareTransfer(source))]);
    if (result.answer !== index) throw new Error('prepared graph answer changed');
  }
  let alias;
  const isolated = isolate(() => {
    alias = { answer: 91 };
    return alias;
  });
  let refused = false;
  try { await owner.ref.call('echo', [transfer(isolated)]); }
  catch (error) { refused = error.name === 'DataCloneError'; }
  if (!refused || alias.answer !== 91) {
    throw new Error('retained sender alias was moved or not refused');
  }
} finally {
  globalThis.setTimeout = timer;
  await owner.stop();
}
if (timerCalls !== 0) throw new Error('prepared graph call scheduled a polling timer');
console.log('prepared graph handoff wakes its caller');
