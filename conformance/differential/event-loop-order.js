// Ordering only. The delays here are all tiny because node sleeps for real and this file is
// diffed against it -- kiln's clock is virtual, so the same program with hour-long delays would
// finish instantly here and take an hour there. That difference is the point of design.md §11 and
// is exactly why it cannot be checked by diffing.
const log = [];
setTimeout(() => log.push('later'), 4);
setTimeout(() => log.push('zero'), 0);
setTimeout(() => log.push('also zero'), 0);
queueMicrotask(() => log.push('micro'));
log.push('sync');
setTimeout(() => console.log(log.join(',')), 6);

// Microtasks drain completely before any timer, and again after each one -- which is why a job
// queued from inside a timer runs before the next timer, however early that one was due.
const order = [];
setTimeout(() => {
	order.push('first timer');
	queueMicrotask(() => order.push('micro from timer'));
}, 1);
setTimeout(() => order.push('second timer'), 2);
setTimeout(() => console.log(order.join(',')), 3);

// A microtask that queues another still drains before anything else.
const chain = [];
queueMicrotask(() => { chain.push(1); queueMicrotask(() => chain.push(2)); });
setTimeout(() => console.log(chain.join(',')), 0);

// Arguments after the delay reach the callback.
setTimeout((a, b) => console.log('args', a, b), 1, 'x', 2);

// A cleared timer never runs, and clearing an id that is gone is harmless.
const cancelled = setTimeout(() => console.log('never'), 1);
clearTimeout(cancelled);
clearTimeout(cancelled);
clearTimeout(9999);

// An interval reschedules itself and can cancel itself from inside.
let ticks = 0;
const interval = setInterval(() => {
	ticks++;
	if (ticks === 3) {
		clearInterval(interval);
		console.log('ticks', ticks);
	}
}, 2);
