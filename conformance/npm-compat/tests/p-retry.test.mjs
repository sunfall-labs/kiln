// kiln eval smoke test (authored for kiln, not from porffor).
import pRetry from 'p-retry';

let attempts = 0;
const result = await pRetry(async () => { attempts++; if (attempts < 3) throw new Error('fail ' + attempts); return 'success'; }, { retries: 5 });
console.log(result, attempts);
let n = 0;
try { await pRetry(async () => { n++; throw new Error('always'); }, { retries: 2 }); } catch (e) { console.log('gave up after', n, 'attempts:', e.message); }
console.log(await pRetry(async () => 42, { retries: 1 }));
