// The member surface of every callable shape, enumerated rather than remembered.
//
// `Function.prototype.toString` was missing from the native backend's callable members for as
// long as it existed. Nothing caught it: `String(f)` went through the text conversion and was
// right, `f.toString()` went through a member lookup and answered `undefined`, and no probe here
// called the method -- so `native.sh` reported 0 diverged while a program got a wrong answer.
//
// A probe that names one member only ever catches that member. This one asks each shape for
// every member a callable is supposed to have and prints what kind of thing came back, so a
// member implemented on one engine and not the other diverges here without anyone having had to
// think of it first. That is the difference between a gate that covers this bug and a gate that
// covers this class of bug.
function declared(a, b) {
	return a + b;
}
const arrow = (x) => x;
class Klass {
	method() {
		return 1;
	}
	static stat() {
		return 2;
	}
}
const literal = {
	meth(z) {
		return z;
	},
};

const shapes = [
	['declared', declared],
	['arrow', arrow],
	['class', Klass],
	['method', Klass.prototype.method],
	['static', Klass.stat],
	['literal-method', literal.meth],
	['bound', declared.bind(null, 1)],
	['nested-bound', declared.bind(null).bind(null)],
	['builtin-method', Math.max],
	['builtin-ctor', Object],
	// A namespace is deliberately absent: `Math` is not callable, and the backend answers an
	// absent member on a built-in with a refusal rather than `undefined` -- on purpose, because
	// it cannot tell "no such property" from "not implemented yet" and the second must be loud.
	// Asking it for `call` would measure that stance rather than this surface.
];

// Every member `Function.prototype` and `Object.prototype` give a callable. A shape that answers
// `undefined` for one the other shapes answer is either a real gap or a real difference, and
// either way the two engines have to agree about it.
const members = [
	'name',
	'call',
	'apply',
	'bind',
	'toString',
	'constructor',
	'hasOwnProperty',
	'isPrototypeOf',
	'propertyIsEnumerable',
	'valueOf',
];

// `length` is asked of the shapes a program writes and not of the built-ins. Every built-in has
// one -- `Math.max.length` is 2 -- and the native backend refuses it, because the arity lives in
// a ~190-line table in the interpreter that the C side has no copy of. That is a refusal rather
// than a wrong answer, so it is a gap in the backlog and not a bug in the results; duplicating
// the table by hand is how the `node:http` slots rotted, so it wants one shared source of truth
// rather than a second list. Recorded in findings/callable-member-surface.js.

for (const [label, value] of shapes) {
	const seen = [];
	for (const member of members) {
		seen.push(member + '=' + typeof value[member]);
	}
	console.log(label, seen.join(' '));
}

for (const [label, value] of shapes) {
	if (label.startsWith('builtin')) {
		continue;
	}
	console.log('length', label, value.length);
}

// ...and that the members which produce text produce the *same* text, not merely the same type.
for (const [label, value] of shapes) {
	console.log('text', label, JSON.stringify(String(value)) === JSON.stringify(value.toString()));
}
