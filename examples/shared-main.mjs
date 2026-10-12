import { spawnActor, prepareTransfer, transfer } from 'kiln:actors';
import { share } from 'kiln:shared';

const owner = await spawnActor({
  module: new URL('./counter.mjs', import.meta.url),
  export: 'Counter', args: [0],
});
try {
  const record = share({ owner: 'Ada' });
  console.log(await owner.ref.call('readOwner', [record])); // Ada
  const source = { owner: 'Grace' };
  const snapshot = prepareTransfer(source);
  source.owner = 'Changed';
  console.log(await owner.ref.call('readOwner', [transfer(snapshot)])); // Grace
} finally {
  await owner.stop();
}
