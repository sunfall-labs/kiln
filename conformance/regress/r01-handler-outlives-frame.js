// A `return` out of a `try` jumps past the `PopHandler` that would have balanced its
// `PushHandler`. If the handler is not dropped with the frame that installed it, a later
// unrelated `throw` lands on a target in a function that has already returned.
function returnsThroughFinally() {
	try {
		return 1;
	} finally {
	}
}

function throwsAfterwards() {
	returnsThroughFinally();
	throw new Error('boom');
}

try {
	console.log('start', returnsThroughFinally());
	throwsAfterwards();
	console.log('not reached');
} catch (error) {
	console.log('caught', error.message);
}
