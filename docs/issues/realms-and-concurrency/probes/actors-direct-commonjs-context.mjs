// Incoming proof must classify the require context as well as ordinary GC object fields.
import { createRequire } from 'node:module';
import { isolate, spawnActor, transfer } from 'kiln:actors';
const require = createRequire(import.meta.url);
const fixture = require('./support/graph-require-context.cjs');
if (fixture.value !== 7) throw new Error('CommonJS fixture changed');
const owner = await spawnActor({
  module: new URL('../../../conformance/native/support/actor-counter.mjs', import.meta.url),
  export: 'Counter', args: [0],
});
try {
  const ticket = transfer(isolate(() => ({ answer: 41 })));
  const reply = await owner.ref.call('echo', [ticket]);
  if (reply.answer !== 41) throw new Error('CommonJS sender direct graph changed');
  console.log('CommonJS sender direct graph moved');
} finally {
  await owner.stop();
}
