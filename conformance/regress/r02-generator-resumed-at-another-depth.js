// A generator's operand stack is restored wherever the resuming caller's stack ends, so a
// handler registered on one resumption is at a different absolute position on the next. This
// resumes at three different depths and then unwinds through the `finally`.
const log = [];
function* counting() {
	try {
		yield 1;
		yield 2;
		yield 3;
	} finally {
		log.push('cleanup');
	}
}

const it = counting();
console.log(it.next().value);
console.log([it.next().value, it.next().value].join(','));
const ended = it.return('early');
console.log(ended.value, ended.done, log.join(','));
