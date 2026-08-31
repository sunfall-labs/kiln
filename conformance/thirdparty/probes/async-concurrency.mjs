// p-queue / p-map / p-limit / p-retry / p-timeout, plus IxJS async-iterable operators.
//
// The promise-and-async-iteration input, and the one aimed squarely at what Test262's
// built-ins/Promise cluster covers. Between them these exercise: `Promise.withResolvers`,
// `Promise.allSettled` / `any` / `race` and the `AggregateError` they build, unhandled-rejection
// timing, `AbortController` + `AbortSignal` wired through `addEventListener`, `queueMicrotask`
// ordering against timers, `Symbol.asyncIterator` on user classes, `for await` over a sync
// iterable of promises, and async generator `return`/`throw` on early exit.
import PQueue from "p-queue";
import pMap from "p-map";
import pLimit from "p-limit";
import pRetry, { AbortError } from "p-retry";
import pTimeout from "p-timeout";
import { AsyncIterableX, from as ixFrom } from "ix/Ix.asynciterable";
import { map as ixMap, filter as ixFilter, flatMap as ixFlatMap } from "ix/Ix.asynciterable.operators";

const lines = [];
const say = (...a) => lines.push(a.join(" "));
const tick = (ms, v) => new Promise((r) => setTimeout(() => r(v), ms));

// Each block is run through `step` so that one missing global reports itself and the remaining
// blocks still run. A probe that stops at the first gap only ever finds one gap.
async function step(name, fn) {
  try { await fn(); } catch (e) { say(name, "THREW", e && (e.constructor ? e.constructor.name : "?"), e && e.message); }
}

async function main() {
  await step("p-limit", async () => {
    // p-limit: a semaphore built on a deferred queue.
    const limit = pLimit(2);
    let live = 0, peak = 0;
    const limited = await Promise.all([5, 4, 3, 2, 1].map((n) => limit(async () => {
      live++; peak = Math.max(peak, live);
      await tick(n);
      live--;
      return n;
    })));
    say("p-limit", limited.join(","), "peak<=2", peak <= 2);
  });

  await step("p-map-array", async () => {
    // p-map: bounded concurrency plus an async iterable input.
    say("p-map-array", (await pMap([1, 2, 3, 4], async (x) => x * 2, { concurrency: 2 })).join(","));
    async function* gen() { for (const x of [1, 2, 3]) { await Promise.resolve(); yield x; } }
    say("p-map-asynciter", (await pMap(gen(), async (x) => x + 10, { concurrency: 2 })).join(","));
  });

  await step("p-queue", async () => {
    // p-queue: an EventEmitter-shaped scheduler with priorities and onIdle.
    const q = new PQueue({ concurrency: 1 });
    const order = [];
    q.add(async () => { order.push("a"); await tick(2); }, { priority: 0 });
    q.add(async () => { order.push("b"); }, { priority: 10 });
    q.add(async () => { order.push("c"); }, { priority: 5 });
    await q.onIdle();
    say("p-queue", order.join(""), "size", q.size, "pending", q.pending);
  });

  await step("p-retry", async () => {
    // p-retry: a rejected promise retried, and AbortError short-circuiting the retry loop.
    let n = 0;
    say("p-retry", await pRetry(async () => { n++; if (n < 3) throw new Error("no"); return "ok@" + n; }, { retries: 5, minTimeout: 1 }));
    try {
      await pRetry(async () => { throw new AbortError("stop"); }, { retries: 5, minTimeout: 1 });
      say("p-retry-abort", "no-throw");
    } catch (e) { say("p-retry-abort", e.message); }
  });

  await step("p-timeout-ok", async () => {
    // p-timeout: races a promise against a timer and must clear it either way.
    say("p-timeout-ok", await pTimeout(tick(1, "fast"), { milliseconds: 200 }));
    try {
      await pTimeout(tick(200, "slow"), { milliseconds: 5 });
      say("p-timeout-late", "no-throw");
    } catch (e) { say("p-timeout-late", e.name); }
  });

  await step("abort", async () => {
    // AbortController threaded through: addEventListener on an AbortSignal.
    const ac = new AbortController();
    const aborted = [];
    ac.signal.addEventListener("abort", () => aborted.push("evt:" + ac.signal.reason));
    ac.abort("why");
    say("abort", ac.signal.aborted, aborted.join(""), typeof AbortSignal.timeout);
  });

  await step("allSettled", async () => {
    // The combinators Test262 spends most of built-ins/Promise on.
    const settled = await Promise.allSettled([Promise.resolve(1), Promise.reject(new Error("e")), tick(1, 3)]);
    say("allSettled", settled.map((s) => s.status + ":" + (s.status === "fulfilled" ? s.value : s.reason.message)).join(","));
    say("any", await Promise.any([Promise.reject(new Error("a")), tick(1, "b")]));
    try {
      await Promise.any([Promise.reject(new Error("x")), Promise.reject(new Error("y"))]);
      say("any-all-reject", "no-throw");
    } catch (e) { say("any-all-reject", e.constructor.name, e.errors.map((x) => x.message).join("")); }
    say("race", await Promise.race([tick(50, "slow"), tick(1, "fast")]));
  });

  await step("withResolvers", async () => {
    // Promise.withResolvers, and a real subclass with a species-driven `then`.
    if (typeof Promise.withResolvers === "function") {
      const { promise, resolve } = Promise.withResolvers();
      resolve("wr");
      say("withResolvers", await promise);
    } else { say("withResolvers", "unsupported"); }

    class Tracked extends Promise {
      static tally = 0;
      constructor(exec) { super(exec); Tracked.tally++; }
    }
    const t = Tracked.resolve(1).then((x) => x + 1);
    say("subclass", await t, t instanceof Tracked, Tracked.tally > 0, Tracked[Symbol.species] === Tracked);
  });

  await step("ordering", async () => {
    // Microtask vs macrotask ordering, which every scheduler above depends on.
    const seq = [];
    await new Promise((resolve) => {
      setTimeout(() => { seq.push("timeout"); resolve(); }, 0);
      queueMicrotask(() => seq.push("micro"));
      Promise.resolve().then(() => seq.push("then"));
    });
    say("ordering", seq.join(">"));
  });

  await step("ix", async () => {
    // IxJS: operators over async iterables, with early exit driving the generator's return path.
    const ix = ixFrom([1, 2, 3, 4, 5]).pipe(ixMap(async (x) => x * 2), ixFilter((x) => x > 4), ixFlatMap((x) => ixFrom([x, -x])));
    const got = [];
    for await (const v of ix) { got.push(v); if (got.length === 4) break; }
    say("ix", got.join(","), ix instanceof AsyncIterableX);
  });

  await step("for-await-sync", async () => {
    // for await over a sync iterable of promises: each element is awaited in turn.
    const awaited = [];
    for await (const v of [Promise.resolve("p1"), tick(1, "p2"), "p3"]) awaited.push(v);
    say("for-await-sync", awaited.join(","));
  });

  await step("asyncgen-finally", async () => {
    // An async generator closed early must run its finally block.
    const closed = [];
    async function* withFinally() { try { yield 1; yield 2; } finally { closed.push("cleanup"); } }
    for await (const v of withFinally()) { closed.push("v" + v); break; }
    say("asyncgen-finally", closed.join(","));
  });

  console.log(lines.join("\n"));
}

main().catch((e) => { console.log("FATAL", e && (e.stack || e.message)); });
