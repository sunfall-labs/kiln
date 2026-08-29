// IteratorClose on every abrupt completion, not only `break`. The porffor corpus this repo's
// guard probes came from closed on `break` alone and accepted that limit deliberately; g18 only
// checks that one path, so the other three are checked here.
function tracked(label, log) {
	let i = 0;
	return {
		[Symbol.iterator]() {
			return {
				next: () => ({ value: ++i, done: i > 3 }),
				return() {
					log.push(label);
					return { done: true };
				},
			};
		},
	};
}

let log = [];
for (const x of tracked('break', log)) break;
console.log('break:', log.join(',') || '(none)');

// Neither staying in the loop nor running out of values closes anything.
log = [];
for (const x of tracked('exhaust', log)) continue;
console.log('exhaust:', log.join(',') || '(none)');

log = [];
(function () {
	for (const x of tracked('return', log)) return;
})();
console.log('return:', log.join(',') || '(none)');

log = [];
try {
	for (const x of tracked('throw', log)) throw new Error('boom');
} catch (error) {
	console.log('throw:', log.join(',') || '(none)', error.message);
}

// The `finally` runs first, then the iterator closes.
log = [];
for (const x of tracked('nested', log)) {
	try {
		break;
	} finally {
		log.push('finally');
	}
}
console.log('nested try:', log.join(','));

// An inner `break` closes only the inner iterator.
log = [];
for (const a of tracked('outer', log)) {
	for (const b of tracked('inner', log)) break;
	break;
}
console.log('nested loops:', log.join(','));
