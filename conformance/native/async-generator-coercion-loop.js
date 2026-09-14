// Coercions in loops around suspension remain observable after universal LICM.
let calls = 0;
const value = {
  valueOf() {
    calls += 1;
    return 2;
  },
};

function* generator() {
  for (let i = 0; i < 2; i += 1) {
    yield i;
    value + 1;
    +value;
    value < 10;
  }
}

async function asynchronous() {
  for (let i = 0; i < 2; i += 1) {
    await Promise.resolve(i);
    value + 1;
    +value;
    value < 10;
  }
  return calls;
}

const yielded = [];
for (const item of generator()) yielded.push(item);
asynchronous().then((result) => console.log(yielded.join(","), result, calls));
