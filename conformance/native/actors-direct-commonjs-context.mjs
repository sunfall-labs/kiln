// kiln-native: stress=skip
// Every-allocation collection invalidates direct factory provenance; direct rollback has a native stress companion.
// Incoming proof must classify the require context as well as ordinary GC object fields.
import { createRequire } from 'node:module';
import { isolate, spawnActor, transfer } from 'kiln:actors';
const require = createRequire(import.meta.url);
const fixture = require('./support/graph-require-context.cjs');
if (fixture.value !== 7) throw new Error('CommonJS fixture changed');
const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter', args: [0],
});
try {
  const ticket = transfer(isolate(() => ({ answer: 41 })));
  const reply = await owner.ref.call('echo', [ticket]);
  if (reply.answer !== 41) throw new Error('CommonJS sender direct graph changed');
  const aliased = transfer(isolate(() => {
    fixture.value = { answer: 43 };
    return fixture.value;
  }));
  let refused = false;
  try { await owner.ref.call('echo', [aliased]); }
  catch (error) { refused = error.name === 'DataCloneError'; }
  if (!refused || fixture.value.answer !== 43) throw new Error('retained CommonJS export moved');
  fixture.value = undefined;
  const retry = await owner.ref.call('echo', [aliased]);
  if (retry.answer !== 43) throw new Error('CommonJS export clear did not permit retry');
  console.log('CommonJS sender direct graph moved; retained export refused');
} finally {
  await owner.stop();
}
