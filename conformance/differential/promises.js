// A promise is a value that arrives once. Its handlers never run synchronously, which is why
// everything below lands after the whole script has finished.
const order = [];
const settled = new Promise((resolve) => resolve('executor'));
settled.then((v) => order.push('then:' + v));
order.push('sync');
Promise.resolve().then(() => console.log(order.join(',')));

// A handler's return value resolves the derived promise, and returning a promise waits for it
// rather than fulfilling with the promise object -- which is what makes chains chain.
Promise.resolve(1)
	.then((v) => v + 1)
	.then((v) => Promise.resolve(v * 10))
	.then((v) => console.log('chain', v));

// A throw in a handler rejects the derived promise; a `catch` recovers and the chain continues.
Promise.resolve()
	.then(() => { throw new Error('thrown'); })
	.catch((e) => 'recovered:' + e.message)
	.then((v) => console.log(v));

// `then` with no rejection handler passes a rejection through; `catch` with no value handler
// passes a value through.
Promise.reject(new Error('passes'))
	.then((v) => console.log('not reached', v))
	.catch((e) => console.log('passed through', e.message));

// `finally` runs either way and does *not* change the outcome, which is the whole difference
// from `then(f, f)`.
Promise.resolve('kept')
	.finally(() => 'ignored')
	.then((v) => console.log('finally kept', v));
Promise.reject(new Error('still rejects'))
	.finally(() => 'ignored')
	.catch((e) => console.log('finally', e.message));

// The combinators.
Promise.all([1, Promise.resolve(2), 3]).then((v) => console.log('all', v.join(',')));
Promise.all([Promise.resolve(1), Promise.reject(new Error('first'))]).catch((e) => console.log('all rejects', e.message));
Promise.race([new Promise(() => {}), Promise.resolve('won')]).then((v) => console.log('race', v));
Promise.allSettled([Promise.resolve(1), Promise.reject('why')]).then((r) => console.log('allSettled', JSON.stringify(r)));
Promise.any([Promise.reject('a'), Promise.resolve('b')]).then((v) => console.log('any', v));
Promise.all([]).then((v) => console.log('empty all', v.length));

// Results keep input order rather than completion order.
const slow = new Promise((resolve) => setTimeout(() => resolve('slow'), 3));
const quick = Promise.resolve('quick');
Promise.all([slow, quick]).then((v) => console.log('ordered', v.join(',')));

// An executor that throws rejects the promise, which is how a rejection is often written.
new Promise(() => { throw new Error('from executor'); }).catch((e) => console.log('executor', e.message));

// Settling twice is ignored: a promise arrives once.
new Promise((resolve, reject) => { resolve('first'); resolve('second'); reject(new Error('no')); })
	.then((v) => console.log('once', v));

// Microtasks and promise reactions share one queue, in order.
const interleaved = [];
Promise.resolve().then(() => interleaved.push('promise'));
queueMicrotask(() => interleaved.push('microtask'));
Promise.resolve().then(() => interleaved.push('promise 2'));
setTimeout(() => console.log('interleaved', interleaved.join(',')), 1);
