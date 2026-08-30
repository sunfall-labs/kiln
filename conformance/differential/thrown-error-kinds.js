// The VM raises real errors a program can catch, rather than failing outright. A fatal error
// would make `try { JSON.parse(input) } catch {}` -- ordinary code -- unwritable.
function caught(run) {
	try {
		run();
		return 'no throw';
	} catch (error) {
		return error.constructor.name + ': ' + (error instanceof Error);
	}
}
console.log(caught(() => null.x));
console.log(caught(() => undefined[0]));
console.log(caught(() => { const f = undefined; f(); }));
console.log(caught(() => { const C = 5; new C(); }));
console.log(caught(() => JSON.parse('{oops')));
console.log(caught(() => (5).toString(99)));
console.log(caught(() => [].reduce((a, b) => a + b)));
console.log(caught(() => { const cycle = {}; cycle.self = cycle; JSON.stringify(cycle); }));

// The subclasses are real constructors with their own prototypes, and every one is an Error.
for (const Kind of [Error, TypeError, RangeError, ReferenceError, SyntaxError]) {
	const instance = new Kind('boom');
	console.log(
		Kind.name,
		instance.name,
		instance.message,
		instance instanceof Kind,
		instance instanceof Error,
		String(instance),
	);
}

// A subclass instance is not an instance of a sibling.
console.log(new TypeError('x') instanceof RangeError, new Error('x') instanceof TypeError);

// A thrown error keeps its identity through the handler.
const thrown = new RangeError('r');
try { throw thrown; } catch (error) { console.log(error === thrown, error.constructor.name); }
