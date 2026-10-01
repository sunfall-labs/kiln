// kiln eval smoke test (authored for kiln, not from porffor).
const invariant = require('tiny-invariant');

invariant(true, 'should not throw');
console.log('truthy passes');
try {
  invariant(false, 'User %s not found', 9);
  console.log('no-throw');
} catch (e) {
  console.log('message:', e.message);
  console.log('is Error:', e instanceof Error);
}
try {
  invariant(0, 'zero is falsy');
} catch (e) {
  console.log('falsy throws:', e.message);
}
try {
  invariant(false);
} catch (e) {
  console.log('no message:', JSON.stringify(e.message));
}
invariant('non-empty string');
console.log('string truthy passes');
