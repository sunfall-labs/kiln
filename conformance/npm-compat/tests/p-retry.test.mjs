// kiln npm functional test: p-retry
import pRetry, { AbortError } from 'p-retry';

// 1. Success on attempt 3 with onFailedAttempt metadata
const failures = [];
let count = 0;
const res = await pRetry(
  async () => {
    count++;
    if (count < 3) throw new Error('attempt-' + count + '-failed');
    return 'recovered';
  },
  {
    retries: 5,
    minTimeout: 0,
    maxTimeout: 0,
    onFailedAttempt: (error) => {
      failures.push({ attempt: error.attemptNumber, left: error.retriesLeft, msg: error.message });
    }
  }
);
console.log('result:', res, 'total-runs:', count);
console.log('failures-logged:', JSON.stringify(failures));

// 2. AbortError immediate termination without retrying
let abortAttempts = 0;
let abortCaught = null;
try {
  await pRetry(
    async () => {
      abortAttempts++;
      throw new AbortError('unrecoverable-auth-error');
    },
    { retries: 10, minTimeout: 0 }
  );
} catch (err) {
  abortCaught = { msg: err.message, name: err.name };
}
console.log('abort-attempts-is-1:', abortAttempts === 1);
console.log('abort-caught:', JSON.stringify(abortCaught));

// 3. Retry exhaustion
let exhaustAttempts = 0;
let exhaustError = null;
try {
  await pRetry(
    async () => {
      exhaustAttempts++;
      throw new Error('persistent-network-issue');
    },
    { retries: 3, minTimeout: 0 }
  );
} catch (err) {
  exhaustError = err.message;
}
console.log('exhaust-attempts:', exhaustAttempts);
console.log('exhaust-error:', exhaustError);
