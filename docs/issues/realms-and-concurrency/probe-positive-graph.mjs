// Native-only issue 10 probe. The interpreter does not move actor graph chunks.
import { spawnActor, isolate, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('../../conformance/native/support/actor-counter.mjs', import.meta.url),
  export: 'Counter', args: [0], mailboxCapacity: 1,
});
try {
  const array = transfer(isolate(() => [3, 5, 8]));
  const movedArray = await owner.ref.call('echo', [array]);
  if (movedArray.join(',') !== '3,5,8') throw new Error('moved array changed');
  console.log('array moved');
  const longer = transfer(isolate(() => [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]));
  const movedLonger = await owner.ref.call('echo', [longer]);
  if (movedLonger.length !== 12 || movedLonger.join(',') !== '1,2,3,4,5,6,7,8,9,10,11,12')
    throw new Error('moved in-block array changed');
  console.log('in-block array moved');
  const side = transfer(isolate(() => Array(128).fill(7)));
  const movedSide = await owner.ref.call('echo', [side]);
  if (movedSide.length !== 128 || movedSide[0] !== 7 || movedSide[127] !== 7 ||
      movedSide.reduce((sum, value) => sum + value, 0) !== 896)
    throw new Error('moved side-backed array changed');
  console.log('side-backed array moved');
  const pair = transfer(isolate(() => [Array(128).fill(1), Array(128).fill(2)]));
  const movedPair = await owner.ref.call('echo', [pair]);
  if (movedPair.length !== 2 || movedPair[0].length !== 128 ||
      movedPair[1].length !== 128 || movedPair[0][127] !== 1 || movedPair[1][127] !== 2)
    throw new Error('moved side-backed array pair changed');
  console.log('side-backed array pair moved');
  const large = transfer(isolate(() => Array(100000).fill(3)));
  const grownLarge = await owner.ref.call('growTransferredArray', [large]);
  if (grownLarge.join(',') !== '200001,3,3,4')
    throw new Error('large side-backed array failed receiver growth');
  console.log('large side-backed array moved');
  const object = transfer(isolate(() => ({ __proto__: null, values: [3, 5, 8], answer: 42 })));
  const movedObject = await owner.ref.call('echo', [object]);
  if (movedObject.answer !== 42 || movedObject.values.join(',') !== '3,5,8')
    throw new Error('moved object graph changed');
  console.log('object moved');
  const ordinary = transfer(isolate(() => ({ values: [3, 5, 8], answer: 42 })));
  const inspected = await owner.ref.call('inspectGraph', [ordinary]);
  if (inspected[0] !== true || inspected[1] !== 42 || inspected[2] !== '3,5,8')
    throw new Error('moved ordinary object lost its prototype or values');
  console.log('ordinary object moved');
  const wide = transfer(isolate(() => ({
    p0: 0, p1: 1, p2: 2, p3: 3, p4: 4, p5: 5,
    p6: 6, p7: 7, p8: 8, p9: 9, p10: 10, p11: 11,
  })));
  const movedWide = await owner.ref.call('echo', [wide]);
  if (Object.keys(movedWide).length !== 12 || movedWide.p0 !== 0 || movedWide.p11 !== 11)
    throw new Error('moved in-block object changed');
  console.log('in-block object moved');
  const dated = transfer(isolate(() => ({ when: new Date(123456) })));
  const movedDated = await owner.ref.call('echo', [dated]);
  if (!(movedDated.when instanceof Date) || movedDated.when.getTime() !== 123456)
    throw new Error('moved Date changed');
  console.log('Date graph moved');
  const directDate = transfer(isolate(() => new Date(654321)));
  const movedDate = await owner.ref.call('echo', [directDate]);
  if (!(movedDate instanceof Date) || movedDate.getTime() !== 654321)
    throw new Error('moved Date root changed');
  console.log('Date root moved');
  let reused = false;
  try { await owner.ref.call('echo', [array]); }
  catch (error) { reused = error.name === 'DataCloneError'; }
  if (!reused) throw new Error('consumed graph ticket was reused');
  console.log('ticket reuse refused');


  let alias;
  const aliased = transfer(isolate(() => { alias = [9, 10]; return alias; }));
  let refused = false;
  try { await owner.ref.call('echo', [aliased]); }
  catch (error) { console.log('alias outcome', error.name); refused = error.name === 'DataCloneError'; }
  if (!refused || alias.join(',') !== '9,10') throw new Error('sender alias was not retained');
  let sideAlias;
  const aliasedSide = transfer(isolate(() => {
    sideAlias = Array(128).fill(9);
    return sideAlias;
  }));
  let sideRefused = false;
  try { await owner.ref.call('echo', [aliasedSide]); }
  catch (error) { sideRefused = error.name === 'DataCloneError'; }
  if (!sideRefused || sideAlias[127] !== 9)
    throw new Error('side-backed sender alias was not retained');
  if (await owner.ref.call('increment', [1]) !== 1)
    throw new Error('actor did not recover after alias refusal');
  console.log('graph array/object moved; sender alias refused');
} finally {
  await owner.stop();
}
