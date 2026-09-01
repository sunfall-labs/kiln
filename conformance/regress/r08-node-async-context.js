// `react-dom/server` stopped while it was still loading, on the two names its top-level `var`
// list constructs: `new util.TextEncoder()` and `new async_hooks.AsyncLocalStorage()`. Both were
// missing, so both read `undefined`, and the message -- "undefined is not a constructor" --
// named neither the module nor the property, which is why it read as a compiler bug for so long.
//
// The storage is the part worth guarding. A store that is only correct synchronously is worse
// than none: it answers on the near side of an `await` and quietly answers `undefined` on the
// far side, which is exactly the shape a request-scoped logger or trace id is used in.
const util = require('util');
const { AsyncLocalStorage } = require('async_hooks');

console.log('encoder', Array.from(new util.TextEncoder().encode('hi')).join(','));

const als = new AsyncLocalStorage();
const other = new AsyncLocalStorage();

console.log('outside', als.getStore());
console.log('sync', als.run({ id: 1 }, () => als.getStore().id));
console.log('restored', als.getStore());
console.log('args', als.run({ n: 5 }, (x, y) => als.getStore().n + x + y, 2, 3));

// A callback that throws must not leave its store in force for whatever catches it.
try {
	als.run('t', () => {
		throw new Error('boom');
	});
} catch (error) {
	console.log('threw', error.message, als.getStore());
}

// Each storage's stack is its own: one storage's `run` has no opinion about another's.
als.run('a', () => {
	other.run('b', () => console.log('two', als.getStore(), other.getStore()));
	console.log('inner', als.getStore(), other.getStore());
	console.log('exit', als.exit(() => als.getStore()), 'back', als.getStore());
});

async function work(tag) {
	console.log(tag, 'entered', als.getStore());
	await null;
	console.log(tag, 'past await', als.getStore());
	await new Promise((resume) => setTimeout(resume, 1));
	console.log(tag, 'past timer', als.getStore());
}

async function main() {
	// The store crosses an `await` because the reaction that resumes the body was queued while
	// the store was in force and carries the context that had it.
	await als.run('A', () => work('A'));
	// A `then` captures where it was attached, not where it runs.
	await als.run('B', () => Promise.resolve().then(() => console.log('then', als.getStore())));
	// A timer captures where it was set.
	await new Promise((done) =>
		als.run('T', () =>
			setTimeout(() => {
				console.log('timer', als.getStore());
				done();
			}, 1),
		),
	);
	// Two runs in flight at once must not see each other's store.
	await Promise.all([als.run('X', () => work('X')), als.run('Y', () => work('Y'))]);
	console.log('done', als.getStore());
}

main();
