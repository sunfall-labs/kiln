// A switch is not a set of alternatives: control enters at the matching arm and keeps going
// through the arms after it until a `break`, so the bodies are one run entered at different
// offsets rather than a dispatch table.
function classify(value) {
	const seen = [];
	switch (value) {
		case 1:
			seen.push('one');
		case 2:
			seen.push('two');
			break;
		case 3:
			seen.push('three');
		default:
			seen.push('default');
		case 4:
			seen.push('four');
	}
	return seen.join(',');
}
console.log(classify(1), '|', classify(2), '|', classify(3), '|', classify(4), '|', classify(9));

// The tests are evaluated in order, and only until one matches.
const evaluated = [];
const probe = (n) => { evaluated.push(n); return n; };
switch (2) {
	case probe(1): break;
	case probe(2): break;
	case probe(3): break;
}
console.log(evaluated.join(','));

// Matching is `===`, so no coercion happens.
switch ('1') {
	case 1: console.log('coerced'); break;
	default: console.log('strict');
}

// `break` leaves the switch; `continue` steps over it to the enclosing loop.
let kept = 0;
for (let i = 0; i < 4; i++) {
	switch (i) {
		case 1:
			continue;
		case 2:
			break;
		default:
			break;
	}
	kept++;
}
console.log(kept);

// A switch whose `break` leaves only the switch, so the loop keeps running.
let total = 0;
for (let i = 0; i < 5; i++) {
	switch (i) {
		case 3:
			break;
		default:
			total += i;
	}
}
console.log(total);

// An empty switch, and one whose only arm is default.
switch (1) {}
switch (1) { default: console.log('only default'); }
