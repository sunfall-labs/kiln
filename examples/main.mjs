import { spawnActor } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('./counter.mjs', import.meta.url),
  export: 'Counter', args: [0], mailboxCapacity: 8,
});
try {
  console.log(await owner.ref.call('increment', [3])); // 3
} finally {
  await owner.stop();
}
