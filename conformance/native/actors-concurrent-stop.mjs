// A scheduled worker's exit request must survive its final turn: sixteen actors finish their
// calls, then all owners stop together. The old launcher read the scheduler lane's empty exit
// slot instead of each realm's saved slot and reported a normal termination as an error.
import { spawnActor } from 'kiln:actors';
import { share } from 'kiln:shared';

const source = { records: [{ n: 2 }, { n: 3 }, { n: 5 }] };
source.alias = source.records[1];
const graph = share(source);
const target = new URL('./support/actor-shared.mjs', import.meta.url);
const owners = [];
try {
  for (let index = 0; index < 16; index++) {
    owners.push(await spawnActor({ module: target, export: 'SharedReader', args: [graph] }));
  }
  const results = await Promise.all(owners.map(owner => owner.ref.call('inspect', [])));
  for (const result of results) {
    if (!result.shared || !result.nested || !result.alias || result.sum !== 10)
      throw new Error('shared actor lost its graph');
  }
} finally {
  await Promise.all(owners.map(owner => owner.stop()));
}
console.log('stopped 16 shared actors');
