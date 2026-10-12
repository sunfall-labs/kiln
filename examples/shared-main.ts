import { spawnActor } from 'kiln:actors';
import { share } from 'kiln:shared';
import type { Counter } from './counter.ts';

const owner = await spawnActor<Counter>({
  module: new URL('./counter.ts', import.meta.url),
  export: 'Counter', args: [0],
});
try {
  const record = share({ owner: 'Ada' });
  console.log(await owner.ref.call('readOwner', [record])); // Ada
  // record.owner = 'Grace'; // Type error: the snapshot is readonly.
} finally {
  await owner.stop();
}
