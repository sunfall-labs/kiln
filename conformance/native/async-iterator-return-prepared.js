const events = [];

function makeIterator(label) {
  let getterReads = 0;
  const iterator = {
    [Symbol.asyncIterator]() { return this; },
    next() { return Promise.resolve({ value: label, done: false }); },
    get return() {
      getterReads++;
      events.push(`${label}:get:${getterReads}`);
      return function () {
        events.push(`${label}:call:${this === iterator}:${arguments.length}`);
        return Promise.resolve().then(() => {
          events.push(`${label}:settled`);
          return { done: true };
        });
      };
    },
    reads() { return getterReads; },
  };
  return iterator;
}

async function main() {
  const breaking = makeIterator("break");
  for await (const value of breaking) {
    events.push(`break:value:${value}`);
    break;
  }
  events.push("break:after");

  const throwing = makeIterator("throw");
  const original = new Error("body");
  let caught;
  try {
    for await (const value of throwing) {
      events.push(`throw:value:${value}`);
      throw original;
    }
  } catch (error) {
    caught = error === original;
  }
  events.push(`throw:caught:${caught}`);

  if (breaking.reads() !== 1 || throwing.reads() !== 1) {
    throw new Error("iterator return getter must be read once per close");
  }
  const expected = [
    "break:value:break",
    "break:get:1",
    "break:call:true:0",
    "break:settled",
    "break:after",
    "throw:value:throw",
    "throw:get:1",
    "throw:call:true:0",
    "throw:settled",
    "throw:caught:true",
  ].join(",");
  if (events.join(",") !== expected) {
    throw new Error(`unexpected iterator close order: ${events.join(",")}`);
  }
  console.log(events.join(","));
}

main().catch((error) => {
  console.log("unexpected", error && (error.stack || error.message));
  throw error;
});
