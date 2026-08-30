// A label is what lets a jump name a construct other than the innermost one -- which is the only
// way to leave two loops with one `break`.
let pairs = 0;
outer: for (let i = 0; i < 3; i++) {
	for (let j = 0; j < 3; j++) {
		if (j === 1) continue outer;
		if (i === 2) break outer;
		pairs++;
	}
}
console.log(pairs);

// `continue outer` re-runs the *outer* update, so the inner loop restarts from scratch.
const visited = [];
first: for (const a of [1, 2]) {
	for (const b of ['x', 'y']) {
		visited.push(String(a) + b);
		continue first;
	}
}
console.log(visited.join(','));

// A label on a non-loop statement makes a block a `break` can leave.
let reached = 'no';
block: {
	reached = 'entered';
	if (true) break block;
	reached = 'not reached';
}
console.log(reached);

// A label on a switch, broken from inside a nested loop.
let found = '';
lookup: switch ('b') {
	case 'b':
		for (const candidate of ['x', 'y']) {
			found = candidate;
			break lookup;
		}
		found = 'not reached';
}
console.log(found);

// An unlabelled `break` inside a labelled loop still leaves only the innermost.
let inner = 0;
tagged: for (let i = 0; i < 2; i++) {
	for (let j = 0; j < 5; j++) {
		if (j === 1) break;
		inner++;
	}
}
console.log(inner);

// A `finally` between the jump and its target still runs.
const order = [];
cleanup: for (let i = 0; i < 2; i++) {
	try {
		break cleanup;
	} finally {
		order.push('finally');
	}
}
console.log(order.join(','));
