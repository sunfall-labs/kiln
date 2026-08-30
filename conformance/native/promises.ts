// A promise is a value that arrives once, later, and everything waiting for it. The ordering
// below is the point: every line after "sync end" runs from the microtask queue, and the order
// they come out in is the order they were queued -- which is what makes the two engines
// comparable at all.
const resolved = Promise.resolve(1);
console.log(resolved, typeof resolved, resolved instanceof Promise);
resolved.then((v: number) => console.log("then", v));
Promise.reject(new Error("no")).catch((e: any) => console.log("caught", e.message));
new Promise<number>((resolve) => resolve(2)).then((v: number) => console.log("exec", v));
new Promise<number>((_, reject) => reject("r")).catch((e: any) => console.log("rej", e));

// A chain: each `then` returns a new promise settled with what the handler produced, and a
// rejection travels down until something catches it.
Promise.resolve(3).then((v: number) => v * 2).then((v: number) => console.log("chain", v));
Promise.reject("early").then((v: any) => console.log("skipped", v)).catch((e: any) => console.log("late", e));

// A handler that throws rejects the promise it was producing, rather than failing the program.
Promise.resolve(1).then(() => { throw new Error("inside"); }).catch((e: any) => console.log("threw", e.message));

// Resolving with a promise adopts it rather than nesting, which is what keeps a chain flat.
Promise.resolve(Promise.resolve("inner")).then((v: string) => console.log("adopted", v));

console.log("sync end");

// `await` is a suspension: the body runs to the first one synchronously and resumes from the
// queue. The generator machinery does the suspending; the promise does the scheduling.
async function go(): Promise<number> {
  const v = await Promise.resolve(7);
  console.log("await", v);
  const w = await 10;
  return v + w;
}
go().then((v: number) => console.log("async result", v));

async function boom(): Promise<number> { throw new Error("async boom"); }
boom().catch((e: any) => console.log("async caught", e.message));

async function rejected(): Promise<number> { return await Promise.reject("from await"); }
rejected().catch((e: any) => console.log("await rejected", e));

// An async function that awaits another one, which is the shape every real program has.
async function inner(): Promise<number> { return await Promise.resolve(5); }
async function outer(): Promise<number> { return (await inner()) * 2; }
outer().then((v: number) => console.log("nested", v));

// `all` waits for every one and keeps their order; `race` takes the first to arrive.
Promise.all([Promise.resolve(1), 2, Promise.resolve(3)]).then((vs: number[]) => console.log("all", vs));
Promise.all([]).then((vs: number[]) => console.log("all empty", vs));
Promise.race([Promise.resolve("first"), Promise.resolve("second")]).then((v: string) => console.log("race", v));
Promise.all([Promise.resolve(1), Promise.reject("bad")]).catch((e: any) => console.log("all rejected", e));
