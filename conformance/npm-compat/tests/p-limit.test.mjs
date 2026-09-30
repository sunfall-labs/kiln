// kiln eval smoke test (authored for kiln, not from porffor).
import pLimit from 'p-limit';

const limit = pLimit(2);
const order = [];
const results = await Promise.all([1, 2, 3, 4].map((i) => limit(async () => { order.push(i); return i * 10; })));
console.log(order.join(','));
console.log(results.join(','));
console.log(limit.activeCount, limit.pendingCount, limit.concurrency);
