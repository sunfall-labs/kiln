// kiln npm functional test: p-limit
import pLimit from 'p-limit';

const limit = pLimit(2);
let active = 0;
let maxActiveObserved = 0;

const runTask = async (id, durationMs, shouldFail = false) => {
  return limit(async () => {
    active++;
    if (active > maxActiveObserved) maxActiveObserved = active;
    await new Promise(r => setTimeout(r, durationMs));
    active--;
    if (shouldFail) throw new Error('task-' + id + '-failed');
    return 'task-' + id + '-ok';
  });
};

// 1. Run 5 tasks through concurrency limit of 2, with 1 task intentionally failing
const promises = [
  runTask(1, 20),
  runTask(2, 30, true),
  runTask(3, 10),
  runTask(4, 25),
  runTask(5, 15)
];

const results = await Promise.allSettled(promises);
console.log('max-active-lte-2:', maxActiveObserved <= 2);
const outcomes = results.map(r => r.status === 'fulfilled' ? r.value : r.reason.message);
console.log('task-outcomes:', JSON.stringify(outcomes));

// 2. Clear queue before pending tasks begin
const slowLimit = pLimit(1);
const slowP1 = slowLimit(async () => {
  await new Promise(r => setTimeout(r, 30));
  return 'slow-1';
});
const slowP2 = slowLimit(async () => 'slow-2-cancelled');
const slowP3 = slowLimit(async () => 'slow-3-cancelled');

console.log('before-clear-pending:', slowLimit.pendingCount);
slowLimit.clearQueue();
console.log('after-clear-pending:', slowLimit.pendingCount);

console.log('slow-p1:', await slowP1);
