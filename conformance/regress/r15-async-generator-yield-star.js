// Async yield* must expose the delegated iterator's value, including promise values,
// rather than exposing the iterator-result object to for-await consumers.
const closed = [];
const delegate = {
  [Symbol.iterator]() {
    let index = 0;
    return {
      next() {
        if (index++ === 0) return { value: 1, done: false };
        if (index === 2) return { value: Promise.resolve(2), done: false };
        return { value: 7, done: true };
      },
      return(value) {
        closed.push(value);
        return { value: 99, done: true };
      },
    };
  },
};

async function* delegated() {
  const completion = yield* delegate;
  return completion + 1;
}

async function main() {
  const values = [];
  for await (const value of delegated()) values.push(value);
  console.log(values.join(','));

  const early = delegated();
  await early.next();
  const returned = await early.return('stop');
  console.log(returned.value, returned.done, closed.join(','));
}

main();
