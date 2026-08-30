// An async function's body is a generator and `await` is its suspension: the same machine, with
// a promise settling where a `next` would be. What the caller sees is a promise.
async function one() { return 1; }
console.log(typeof one().then, one() instanceof Promise);

async function chain() {
	const a = await one();
	const b = await Promise.resolve(a + 1);
	return b * 10;
}
chain().then((v) => console.log('chain', v));

// A rejection is raised *at* the await, so an ordinary `try` catches it -- which is the entire
// reason `await` reads like synchronous code.
async function catches() {
	try {
		await Promise.reject(new Error('rejected'));
		return 'not reached';
	} catch (error) {
		return 'handled:' + error.message;
	} finally {
		console.log('finally ran');
	}
}
catches().then((v) => console.log(v));

// A throw anywhere in the body rejects the returned promise.
async function fails() { throw new Error('inside'); }
fails().catch((e) => console.log('caught', e.message));

// Awaiting a plain value still suspends, so everything below runs first.
const order = [];
async function ordering() {
	order.push('before');
	await 1;
	order.push('after');
}
ordering().then(() => console.log('order', order.join(',')));
order.push('sync');

// Async arrows and async methods.
const arrow = async (n) => n * 2;
arrow(21).then((v) => console.log('arrow', v));

// Awaiting in a loop sequences the work.
async function sequence() {
	const seen = [];
	for (const n of [1, 2, 3]) {
		seen.push(await Promise.resolve(n));
	}
	return seen.join(',');
}
sequence().then((v) => console.log('sequence', v));

// An async function that awaits another one, and a rejection propagating through two levels.
async function inner() { throw new Error('deep'); }
async function outer() { await inner(); }
outer().catch((e) => console.log('propagated', e.message));

// `Promise.all` over async calls runs them concurrently and keeps input order.
async function delayed(label, ms) {
	await new Promise((resolve) => setTimeout(resolve, ms));
	return label;
}
Promise.all([delayed('slow', 4), delayed('quick', 1)]).then((v) => console.log('concurrent', v.join(',')));
