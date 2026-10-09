const token = { message: 'same rejection object' };
function* sync() { yield 42; }
async function* delegated() { yield* sync(); }
const iterator = delegated();
console.log('first', JSON.stringify(await iterator.next()));
try {
  const pending = iterator.throw(token);
  console.log('promise', pending instanceof Promise);
  try {
    await pending;
    console.log('unexpected fulfillment');
  } catch (error) {
    console.log('rejection identity', error === token);
  }
  const after = await iterator.next();
  console.log('completed', after.done, after.value === undefined);
} catch (error) {
  console.log('unexpected synchronous throw', error === token);
}
