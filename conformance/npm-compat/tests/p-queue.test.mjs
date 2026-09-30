// kiln eval smoke test (authored for kiln, not from porffor).
import PQueue from 'p-queue';

const queue = new PQueue({ concurrency: 1 });
const results = [];
queue.add(() => results.push('a'));
queue.add(() => results.push('b'));
queue.add(async () => results.push(await Promise.resolve('c')));
await queue.onIdle();
console.log(results.join(','));
console.log(queue.size, queue.pending);
console.log(queue.concurrency);
