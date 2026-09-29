import { spawnActor } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter',
  args: [0],
});

const category = { code: 7 };
const graph = [{ id: 1, category }, { id: 2, category }];
graph.push(graph);
const received = await owner.ref.call('echo', [graph]);
if (received === graph || received[0].category !== received[1].category ||
    received[2] !== received || received[0].id !== 1) {
  throw new Error('plain actor graph lost clone identity or its cycle');
}

let getterReads = 0;
const accessor = {};
Object.defineProperty(accessor, 'answer', {
  enumerable: true,
  get() { getterReads++; return 42; },
});
const observed = await owner.ref.call('echo', [accessor]);
if (observed.answer !== 42 || getterReads !== 1) {
  throw new Error('actor preparation read an accessor more than once');
}

let proxyReads = 0;
const proxy = new Proxy({ answer: 17 }, {
  get(target, key, receiver) {
    if (key === 'answer') proxyReads++;
    return Reflect.get(target, key, receiver);
  },
});
const proxied = await owner.ref.call('echo', [proxy]);
if (proxied.answer !== 17 || proxyReads !== 1) {
  throw new Error('actor preparation read a Proxy more than once');
}

await owner.stop();
console.log('plain actor graph and guarded fallbacks');
