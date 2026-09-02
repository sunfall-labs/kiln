// `new Promise` checks its executor before it builds anything.
//
// The native backend was calling whatever it was handed as the executor -- `undefined` when there
// was no argument at all -- so the TypeError the specification requires never happened, and the
// caller got back a promise that could never settle. That is a wrong answer rather than a missing
// one: no refusal, no diagnostic, and the program continues on a value that will never resolve.
// The order matters and is observable: the throw comes before the promise exists.

for (const executor of [undefined, null, 42, "resolve", {}]) {
  try {
    const made = new Promise(executor as never);
    console.log("built", typeof made);
  } catch (error) {
    console.log("threw", (error as Error).constructor.name);
  }
}

// The ordinary case still works, and an executor that throws rejects rather than failing the
// program -- `new Promise(() => { throw e })` is how a rejection is often written.
new Promise<string>((resolve) => resolve("resolved")).then((value) => console.log("then", value));
new Promise((_, reject) => reject(new Error("rejected"))).catch((error) =>
  console.log("catch", (error as Error).message),
);
new Promise(() => {
  throw new Error("thrown");
}).catch((error) => console.log("thrown-becomes-rejection", (error as Error).message));
