// Native direct-transfer scaling: hold unrelated sender state while handing off a fixed graph.
// Each invocation owns one size/mode; otherwise earlier heaps would contaminate later rows.
import { isolate, spawnActor, transfer } from 'kiln:actors';
import { arg, emit, median, now } from './common.mjs';

const mode = String(arg(0, 'objects'));
const count = Number(arg(1, 10000));
const samples = Number(arg(2, 31));
if (!['objects', 'keys', 'shapes'].includes(mode) || !Number.isInteger(count) || count < 0 ||
    !Number.isInteger(samples) || samples < 3) throw new Error('invalid scaling arguments');
const retained = [];
let expected = 0;
for (let index = 0; index < count; index++) {
  const value = { marker: index };
  if (mode !== 'objects') value['unrelated-key-' + index] = index;
  // Different pairs of names create distinct shape paths as well as distinct keys.
  if (mode === 'shapes') value['unrelated-second-' + index] = index + 1;
  retained.push(value);
  expected += index;
}
const owner = await spawnActor({
  module: new URL('./actor-transfer-scaling-target.mjs', import.meta.url),
  export: 'TransferReader',
});
const elapsed = [];
try {
  for (let index = 0; index < samples + 3; index++) {
    const handle = isolate(() => ({ answer: 42 }));
    const start = now();
    const answer = await owner.ref.call('answer', [transfer(handle)]);
    const duration = now() - start;
    if (answer !== 42) throw new Error('direct-transfer checksum changed');
    if (index >= 3) elapsed.push(duration);
  }
  let observed = 0;
  for (const value of retained) observed += value.marker;
  if (observed !== expected) throw new Error('unrelated sender state changed');
  emit({ case: 'actor-transfer-scaling', mode, unrelated_count: count,
    samples, ms_per_op: median(elapsed), checksum: '42:' + observed });
} finally {
  await owner.stop();
}
