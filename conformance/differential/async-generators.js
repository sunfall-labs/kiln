// An async generator both yields and awaits, and its driver has to tell the two apart.
async function* counted(limit) {
  for (let n = 1; n <= limit; n += 1) {
    // An await between yields is invisible to the consumer: the step is not finished.
    const doubled = await Promise.resolve(n * 2);
    yield doubled;
  }
}

// Calling one runs nothing; the object is what comes back.
const it = counted(3);
console.log(typeof it.next, typeof it[Symbol.asyncIterator]);

async function collect(source) {
  const out = [];
  for await (const value of source) {
    out.push(value);
  }
  return out.join(",");
}

// `next()` hands back a promise of the step, one at a time.
async function stepByHand() {
  const each = counted(2);
  const first = await each.next();
  const second = await each.next();
  const third = await each.next();
  return [first.value, first.done, second.value, third.value, third.done].join("|");
}

// A throw inside the body rejects the step the consumer is waiting on.
async function* fails() {
  yield 1;
  throw new Error("stopped");
}

async function catches() {
  try {
    await collect(fails());
  } catch (error) {
    return "caught " + error.message;
  }
  return "none";
}

// Interleaving proves the awaits actually suspend rather than running straight through.
async function interleaved() {
  const order = [];
  const consume = (async () => {
    for await (const value of counted(2)) {
      order.push("v" + value);
    }
  })();
  order.push("sync");
  await consume;
  return order.join(",");
}

async function main() {
  console.log(await collect(counted(4)));
  console.log(await stepByHand());
  console.log(await catches());
  console.log(await interleaved());
}
main();
