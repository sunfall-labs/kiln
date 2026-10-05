// kiln npm functional test: p-queue
import PQueue from 'p-queue';

const queue = new PQueue({ concurrency: 1 });
const events = [];

queue.on('active', () => events.push('event:active'));
queue.on('completed', (result) => events.push('event:completed:' + result));

// 1. Priority scheduling: lower priority queued first, higher queued second
const executionOrder = [];
queue.pause();

queue.add(async () => {
  executionOrder.push('low');
  return 'L';
}, { priority: 0 });

queue.add(async () => {
  executionOrder.push('high');
  return 'H';
}, { priority: 10 });

queue.add(async () => {
  executionOrder.push('medium');
  return 'M';
}, { priority: 5 });

console.log('paused-size:', queue.size, 'paused-pending:', queue.pending);
queue.start();
await queue.onIdle();

console.log('priority-order:', JSON.stringify(executionOrder));
console.log('events-emitted:', JSON.stringify(events));

// 2. Error resilience in task execution
const results = [];
try {
  await queue.add(async () => {
    throw new Error('task-failed');
  });
} catch (err) {
  results.push('caught:' + err.message);
}
const nextRes = await queue.add(async () => 'next-task-ok');
results.push('after-error:' + nextRes);
console.log('error-resilience:', JSON.stringify(results));

// 3. Clear queue
queue.pause();
queue.add(async () => 'never-runs');
queue.add(async () => 'never-runs-2');
console.log('before-clear-size:', queue.size);
queue.clear();
console.log('after-clear-size:', queue.size);
queue.start();
await queue.onIdle();
console.log('all-done');
