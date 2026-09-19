// Async orchestration: Promise.all/allSettled/race, async iteration, timers, AbortController, queueMicrotask ordering, error handling.
import { setTimeout as sleep, setImmediate as tick } from "node:timers/promises";
const log = [];
queueMicrotask(() => log.push("micro")); process.nextTick(() => log.push("tick")); setTimeout(() => log.push("timer"), 0); Promise.resolve().then(() => log.push("then")); log.push("sync");
await sleep(5); console.log("order:", log.join(">"));
async function job(id, ms, fail = false) { await sleep(ms); if (fail) throw new Error(`job ${id} failed`); return id * 10; }
console.log("all:", (await Promise.all([job(1, 3), job(2, 1), job(3, 2)])).join(","));
const settled = await Promise.allSettled([job(4, 1), job(5, 1, true)]); console.log("settled:", settled.map((s) => s.status + (s.reason ? ":" + s.reason.message : ":" + s.value)).join(" "));
console.log("race:", await Promise.race([job(6, 5), job(7, 1)]), "| any:", await Promise.any([job(8, 2, true), job(9, 1)]));
async function* pages(n) { for (let i = 1; i <= n; i++) { await sleep(1); yield { page: i, items: [i, i * 2] }; } }
let items = 0; for await (const p of pages(3)) items += p.items.length; console.log("pages:", items);
const ac = new AbortController(); const slow = sleep(1000, "late", { signal: ac.signal }).catch((e) => e.name); setTimeout(() => ac.abort(), 2); console.log("abort:", await slow, ac.signal.aborted, ac.signal.reason?.name);
const results = await Promise.all(Array.from({ length: 5 }, (_, i) => tick(i))); console.log("immediate:", results.join(""));
try { await job(10, 1, true); } catch (e) { console.log("caught:", e.message, e instanceof Error, typeof e.stack); }
const withTimeout = (p, ms) => Promise.race([p, sleep(ms).then(() => { throw new Error("timeout"); })]); console.log("timeout:", await withTimeout(job(11, 50), 5).catch((e) => e.message), await withTimeout(job(12, 1), 50));
let closed = false; const it = pages(5)[Symbol.asyncIterator](); await it.next(); await it.return(); closed = true; console.log("early return:", closed);
