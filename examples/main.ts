import { spawnActor } from 'kiln:actors';
import type { Counter } from './counter.ts';

const owner = await spawnActor<Counter>({
  module: new URL('./counter.ts', import.meta.url),
  export: 'Counter', args: [0], mailboxCapacity: 8,
});
try {
  const count: number = await owner.ref.call('increment', [3]);
  console.log(count); // 3
} finally {
  await owner.stop();
}
