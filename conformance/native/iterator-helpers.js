// `Iterator.from` and the `Iterator.prototype` helpers (ES2025).
//
// The point of the family is that it is *lazy*: `map` and `filter` hand back another iterator
// that pulls one element when it is asked, so a pipeline over a large source touches only the
// elements it hands on. That is not a performance note, it is the observable contract -- and it
// is the one a wrong implementation gets wrong silently, by walking the whole source and still
// printing the right answer. So every laziness assertion here counts *pulls* rather than
// results, and the sources are large enough (1000) that a helper which walked them all would
// report a different number rather than merely being slower.
//
// A bundled `immer` reads `Iterator.from` and a bundled `mobx` reads `Iterator.prototype`; those
// two reads are what this file exists for, and everything after them is the surface behind them.
//
// Nothing here probes a member the native backend refuses (`chunks`, `windows`, `join`,
// `Iterator.concat`, `zip`, `zipKeyed`): a refusal ends the program, and it would take every
// assertion after it with it.

const state = { pulled: 0, closed: 0 };

function counting(limit) {
	let at = 0;
	return {
		next() {
			state.pulled += 1;
			return at < limit ? { value: at++, done: false } : { value: undefined, done: true };
		},
		return() {
			state.closed += 1;
			return { value: undefined, done: true };
		},
	};
}

function reset() {
	state.pulled = 0;
	state.closed = 0;
}

console.log("typeof", typeof Iterator, typeof Iterator.from, typeof Iterator.prototype);
console.log("proto-members", typeof Iterator.prototype.map, typeof Iterator.prototype.toArray);
console.log("proto-constructor", Iterator.prototype.constructor === Iterator);

// `Iterator.from` over an object that only ever implemented the protocol: the whole reason the
// function exists, and the shape `immer` hands it.
reset();
const wrapped = Iterator.from(counting(3));
console.log("from-shape", typeof wrapped.next, typeof wrapped.map, typeof wrapped.toArray);
console.log("from-constructor", wrapped.constructor === Iterator);
console.log("from-self", wrapped[Symbol.iterator]() === wrapped);
console.log("from-untouched", state.pulled);
console.log("from-toArray", JSON.stringify(wrapped.toArray()), state.pulled);

// An array is iterable rather than an iterator, so `from` reaches for its `@@iterator` first.
console.log("from-array", JSON.stringify(Iterator.from([1, 2, 3]).toArray()));

// LAZINESS. A thousand available, three asked for, three pulled -- and the source is closed on
// the way out because `take` running out is an early end, not an exhausted one.
reset();
const lazy = Iterator.from(counting(1000)).map((x) => x * 2);
console.log("map-built", state.pulled, state.closed);
console.log("map-take", JSON.stringify(lazy.take(3).toArray()), state.pulled, state.closed);

// `filter` pulls until the predicate says yes, and no further.
reset();
const evens = Iterator.from(counting(1000)).filter((x) => x % 2 === 0);
console.log("filter-first", JSON.stringify(evens.next()), state.pulled);
console.log("filter-second", JSON.stringify(evens.next()), state.pulled);

// `drop` defers its skip to the first `next`, so building one advances nothing.
reset();
const dropped = Iterator.from(counting(1000)).drop(2);
console.log("drop-built", state.pulled);
console.log("drop-take", JSON.stringify(dropped.take(2).toArray()), state.pulled);

// `flatMap` drains an inner iterator before asking the outer one again.
reset();
const flat = Iterator.from(counting(1000)).flatMap((x) => [x, x * 10]);
console.log("flatMap", JSON.stringify(flat.take(5).toArray()), state.pulled);

// WHAT A `flatMap` MAPPER MAY ANSWER WITH, both directions. This is not padding: the first
// version of the native helpers accepted only four value shapes and left arrays out, so the
// `flatMap` line above refused -- and an array is what a mapper returns in almost every real
// program. Pinning the rejecting direction matters at least as much, because a primitive
// accepted as an iterator reads `done` as `undefined` and then loops forever.
console.log(
	"flatMap-set",
	JSON.stringify(
		Iterator.from(counting(2))
			.flatMap((x) => new Set([x, x + 100]))
			.toArray(),
	),
);
function* pair(x) {
	yield x;
	yield x + 1;
}
console.log(
	"flatMap-generator",
	JSON.stringify(Iterator.from(counting(2)).flatMap(pair).toArray()),
);
// A string is iterable and is refused anyway: flattening a returned word into its characters is
// the surprise the specification's reject-primitives rule exists to prevent.
try {
	Iterator.from(counting(2))
		.flatMap(() => "ab")
		.toArray();
	console.log("flatMap-string", "no throw");
} catch (error) {
	console.log("flatMap-string", error instanceof TypeError);
}
try {
	Iterator.from(counting(2))
		.flatMap((x) => x)
		.toArray();
	console.log("flatMap-number", "no throw");
} catch (error) {
	console.log("flatMap-number", error instanceof TypeError);
}

// The callback's second argument is a counter over the elements the helper saw.
console.log(
	"map-index",
	JSON.stringify(
		Iterator.from(counting(3))
			.map((x, i) => x * 100 + i)
			.toArray(),
	),
);

// EXHAUSTION. A helper that ran out answers done for ever, and does not reach back into a source
// that is already finished.
reset();
const short = Iterator.from(counting(1)).map((x) => x);
console.log("exhaust-1", JSON.stringify(short.next()));
console.log("exhaust-2", JSON.stringify(short.next()), state.pulled);
console.log("exhaust-3", JSON.stringify(short.next()), state.pulled);

// EARLY RETURN. Breaking out of a `for...of` calls `return` on the helper, and the close has to
// travel down the pipeline to the source that actually holds something.
reset();
let seen = 0;
for (const value of Iterator.from(counting(1000)).map((x) => x + 1)) {
	seen += value;
	if (value === 3) {
		break;
	}
}
console.log("early-break", seen, state.pulled, state.closed);

// A helper is its own iterable, which is what makes spread work on one.
console.log(
	"spread",
	JSON.stringify([...Iterator.from(counting(4)).filter((x) => x > 1)]),
);

// THE TERMINALS. Each drives the source to its end, or to the element that answers the question.
console.log("toArray", JSON.stringify(Iterator.from(counting(4)).toArray()));
console.log("reduce-seed", Iterator.from(counting(4)).reduce((a, b) => a + b, 100));
console.log("reduce-bare", Iterator.from(counting(4)).reduce((a, b) => a + b));

reset();
console.log("some", Iterator.from(counting(1000)).some((x) => x === 2), state.pulled, state.closed);
reset();
console.log("every", Iterator.from(counting(1000)).every((x) => x < 2), state.pulled, state.closed);
reset();
console.log("find", Iterator.from(counting(1000)).find((x) => x === 3), state.pulled, state.closed);
reset();
console.log(
	"includes",
	Iterator.from(counting(1000)).includes(2),
	state.pulled,
	state.closed,
);
console.log("includes-missing", Iterator.from(counting(3)).includes(9));

let total = 0;
Iterator.from(counting(4)).forEach((x) => {
	total += x;
});
console.log("forEach", total);

// `reduce` over an empty source with no seed has nothing to answer with.
try {
	Iterator.from(counting(0)).reduce((a, b) => a + b);
	console.log("reduce-empty", "no throw");
} catch (error) {
	console.log("reduce-empty", error instanceof TypeError);
}

// A bad argument is settled before the source is stepped, and closes it on the way out.
reset();
try {
	Iterator.from(counting(3)).map(7);
	console.log("map-not-callable", "no throw");
} catch (error) {
	console.log("map-not-callable", error instanceof TypeError, state.pulled, state.closed);
}
reset();
try {
	Iterator.from(counting(3)).take(-1);
	console.log("take-negative", "no throw");
} catch (error) {
	console.log("take-negative", error instanceof RangeError, state.pulled, state.closed);
}

// `take(0)` never touches the source at all, and closes it rather than abandoning it.
reset();
console.log(
	"take-zero",
	JSON.stringify(Iterator.from(counting(1000)).take(0).toArray()),
	state.pulled,
	state.closed,
);
