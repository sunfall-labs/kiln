// kiln eval smoke test (authored for kiln, not from porffor).
const warning = require('warning');

const messages = [];
const origError = console.error;
console.error = (msg) => messages.push(msg);
try {
  warning(true, 'should not print');
  warning(false, 'hello %s', 'world');
  warning(false, 'two args: %d and %s', 1, 'x');
} finally {
  console.error = origError;
}
console.log(messages.length);
console.log(messages[0].includes('hello world'));
console.log(messages[1].includes('1 and x'));
