import { prepareTransfer, spawnActor, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter',
  args: [1],
});

try {
  const wide = new Array(130);
  wide[129] = 42;
  const source = new Array(3);
  source[1] = { wide };
  const handle = prepareTransfer(source);
  wide[129] = 7;
  source[0] = 'after preparation';

  const result = await owner.ref.call('echo', [transfer(handle)]);
  if (!Array.isArray(result) || result.length !== 3 || 0 in result || !(1 in result) ||
      2 in result || !Array.isArray(result[1].wide) || result[1].wide.length !== 130 ||
      0 in result[1].wide || 128 in result[1].wide || !(129 in result[1].wide) ||
      result[1].wide[129] !== 42 || wide[129] !== 7 || source[0] !== 'after preparation') {
    throw new Error('prepared sparse array lost holes or snapshot isolation');
  }
  const ordinarySource = new Array(3);
  ordinarySource[1] = 7;
  const ordinary = await owner.ref.call('echo', [ordinarySource]);
  if (ordinary.length !== 3 || 0 in ordinary || !(1 in ordinary) || 2 in ordinary ||
      ordinary[1] !== 7) {
    throw new Error('ordinary actor message lost array holes');
  }
} finally {
  await owner.stop();
}
console.log('prepared sparse arrays keep holes');
