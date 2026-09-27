// Many pending async tasks in one realm, and the pieces a pending task is made of: a timer whose
// callback is the function itself (a promise's resolve, a closure, a bound function) or carries
// arguments, a promise's first reaction and the ones after it, a suspended frame's locals, and a
// `Promise.all` element step. Order is the contract: timers by deadline and then by arming order,
// reactions in registration order. No wall-clock reading is part of it.
const order = [];
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// Hundreds of tasks on a handful of delays: each delay's timers run in the order they were armed,
// and the delays interleave by deadline. (Few enough that the stressed run, which collects at
// every allocation, stays in seconds; the benchmark is where the count is large.)
async function task(id, delay, rounds) {
  let local = id * 3;
  for (let round = 0; round < rounds; round += 1) {
    await sleep(delay);
    local += round + 1;
  }
  return local;
}

// An arrow created inside a function captures that call's `arguments` only when it can read it.
function outer(a, b) {
  const reads = () => arguments.length + ":" + arguments[0] + ":" + arguments[1];
  const ignores = (x) => x * 2;
  return reads() + "/" + ignores(b);
}

// Timer callbacks of every shape, and one cleared from inside another.
function timerShapes() {
  return new Promise((done) => {
    const seen = [];
    const bound = function (tag) { seen.push(this.name + ":" + tag); }.bind({ name: "bound" }, "b");
    setTimeout(bound, 2);
    setTimeout((tag, n) => seen.push("args:" + tag + n), 2, "a", 1);
    let cancelled = setTimeout(() => seen.push("cancelled"), 3);
    setTimeout(() => { clearTimeout(cancelled); seen.push("clear"); }, 2);
    let count = 0;
    const interval = setInterval(() => {
      count += 1;
      seen.push("interval" + count);
      if (count === 3) clearInterval(interval);
    }, 1);
    setTimeout(() => done(seen.join(",")), 6);
  });
}

// One reaction, then several: registration order survives the move out of the inline slot.
function reactions() {
  let resolve;
  const p = new Promise((r) => { resolve = r; });
  const seen = [];
  for (let i = 0; i < 6; i += 1) p.then((v) => seen.push(v + i));
  const q = p.then((v) => v * 10);
  resolve(1);
  return Promise.all([p, q]).then(([a, b]) => seen.join(",") + "|" + a + "|" + b);
}

function* counter(start) {
  let a = start;
  let b = start * 2;
  while (a < start + 3) {
    const sent = yield a + b;
    a += 1;
    b += sent === undefined ? 0 : sent;
  }
  return a + b;
}

async function* ticks(n) {
  for (let i = 0; i < n; i += 1) {
    await sleep(1);
    yield i;
  }
}

async function main() {
  const delays = [1, 2, 5, 10];
  const running = [];
  for (let i = 0; i < 300; i += 1) {
    const delay = delays[i % delays.length];
    running.push(task(i, delay, 3).then((value) => {
      if (i % 37 === 0) order.push(i + "@" + delay);
      return value;
    }));
  }
  const values = await Promise.all(running);
  let sum = 0;
  for (const value of values) sum += value;
  console.log("tasks", values.length, sum, order.join(" "));

  console.log("arguments", outer(7, 8));
  console.log("timers", await timerShapes());
  console.log("reactions", await reactions());

  const gen = counter(4);
  const stepped = [gen.next().value, gen.next(1).value, gen.next(2).value, gen.next(3).value];
  console.log("generator", stepped.join(","));
  const produced = [];
  for await (const value of ticks(4)) produced.push(value);
  console.log("async generator", produced.join(","));

  const settled = await Promise.allSettled([sleep(1).then(() => "ok"), Promise.reject(new Error("no")), 5]);
  console.log("allSettled", settled.map((r) => r.status + ":" + (r.value ?? r.reason.message)).join(","));
  const first = await Promise.any([Promise.reject(1), sleep(2).then(() => "late"), sleep(1).then(() => "early")]);
  const raced = await Promise.race([sleep(3).then(() => "slow"), sleep(1).then(() => "fast")]);
  console.log("any/race", first, raced);
  try {
    await Promise.all([sleep(1), Promise.reject(new Error("all-rejects")), sleep(2)]);
  } catch (error) {
    console.log("all rejected", error.message);
  }
}

main();
