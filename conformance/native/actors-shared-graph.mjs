import { spawnActor } from 'kiln:actors';
import { isShared, share } from 'kiln:shared';

const source = { records: [{ n: 2 }, { n: 3 }, { n: 5 }] };
source.alias = source.records[1];
const shared = share(source);
source.records[1].n = 99;

const owner = await spawnActor({
  module: new URL('./support/actor-shared.mjs', import.meta.url),
  export: 'SharedReader',
  args: [shared],
});
try {
  for (const result of [await owner.ref.call('inspect', []),
                        await owner.ref.call('inspect', [shared])]) {
    if (!result.shared || !result.nested || result.length !== 3 ||
        result.sum !== 10 || !result.alias) {
      throw new Error('actor did not receive the frozen graph');
    }
  }
  const echoed = await owner.ref.call('echo', [shared]);
  if (!isShared(echoed) || !isShared(echoed.records) ||
      echoed.records[1].n !== 3 || echoed.alias !== echoed.records[1]) {
    throw new Error('actor reply lost the frozen graph');
  }
} finally {
  await owner.stop();
}
console.log('actor shares a frozen graph');
