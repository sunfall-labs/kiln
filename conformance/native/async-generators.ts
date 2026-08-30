// An async generator both yields and awaits, and its driver has to tell the two apart: an `await`
// between two `yield`s is invisible to the consumer, because the step is not finished. The two
// suspensions emit identically -- both save the frame and hand back the same status -- so the
// frame records which one happened. That flag is the whole feature.

async function* counted(limit: number): AsyncGenerator<number> {
  for (let n = 1; n <= limit; n += 1) {
    const doubled = await Promise.resolve(n * 2);
    yield doubled;
  }
}

// Calling one runs nothing; the object is what comes back, and it is its own async iterator.
const made = counted(3);
console.log(typeof made.next, typeof (made as any)[Symbol.asyncIterator]);

async function collect(source: AsyncIterable<number>): Promise<string> {
  const out: number[] = [];
  for await (const value of source) { out.push(value); }
  return out.join(",");
}


// `next()` hands back a promise of the step, one at a time, and the last one is done.
async function stepByHand(): Promise<string> {
  const each = counted(2);
  const first = await each.next();
  const second = await each.next();
  const third = await each.next();
  return [first.value, first.done, second.value, third.value, third.done].join("|");
}


// `for await` over a *synchronous* iterable works too, which is why the loop asks for
// `@@asyncIterator` first and falls back -- and a value without one has to answer that it has
// none rather than failing, which is a protocol probe rather than a missing method.
async function overPlain(): Promise<string> {
  const out: number[] = [];
  for await (const value of [1, 2, 3]) { out.push(value); }
  for await (const value of [Promise.resolve(4), 5]) { out.push(value as number); }
  return out.join(",");
}


// A throw out of the body rejects the step it was in, and the generator is *finished* afterwards
// -- a later `next` answers `{ done: true }` rather than resuming and throwing the same thing
// again, which is what turns one caught rejection into an uncaught one two lines later.
async function* fails(): AsyncGenerator<string> {
  yield "one";
  throw new Error("stopped");
}
async function afterThrowing(): Promise<string> {
  const each = fails();
  const first = (await each.next()).value;
  let caught = "none";
  try { await each.next(); } catch (error: any) { caught = error.message; }
  return [first, caught, (await each.next()).done].join("|");
}

// An async generator that never awaits is still an async generator: its `next` is a promise.
async function* plainSteps(): AsyncGenerator<number> { yield 1; yield 2; }
// Sequenced in one place rather than started in parallel: how many turns each step costs is a
// question both engines answer the same way and node does not, and this probe is about the
// *values*. The ordering gap is written down in NOTES.md rather than asserted here.
(async () => {
  console.log("collected", await collect(counted(3)));
  console.log("by hand", await stepByHand());
  console.log("plain", await overPlain());
  console.log("no awaits", await collect(plainSteps()));
  console.log("after throwing", await afterThrowing());
})();
