// The timing harness every microbenchmark in this directory runs through.
//
// Two artifacts ruined the comparison this replaces, and both are structural rather than bad
// luck on the day:
//
//   * Process startup. node needs about 45ms to boot where a kiln native binary needs 2.6ms, so
//     a benchmark whose work takes 1ms was mostly measuring launchers. Subtracting a floor is
//     not enough on its own -- see the second one.
//   * Warm-up. A JIT has barely started after a single call, so timing one run compares warm
//     ahead-of-time code against cold interpreted code and reports the difference as a property
//     of the compiler. `array-functional` is 4.8x faster on node and 6.9x faster on bun between
//     its first call and its steady state. Measured once, kiln's native backend looked 1.3x
//     slower than node on it; measured warm, it is 11x. The first number was not noise, it was
//     the wrong question.
//
// So the workload runs many times inside one process, the first call is reported apart from the
// rest, and a batch size is calibrated so that one sample comfortably outlasts the clock's
// resolution.
//
// One runtime cannot do that at all. The interpreter's realm time is virtual and advances only
// for scheduled work (ADR 0044), so every clock a program can read -- `Date.now`,
// `performance.now`, `process.hrtime` -- returns the same number from the first line of a
// synchronous run to the last. That is deliberate and worth keeping: it is what makes a run
// reproducible. This harness detects it and switches to a mode where it only counts iterations
// and the *runner* times the process from outside, which is accurate enough there because the
// interpreter's workloads run for tens to thousands of milliseconds against a 7ms floor.
//
// `Date.now` rather than `performance.now` deliberately: both are sub-millisecond in both
// engines now, and `Date.now` is the one every runtime here agrees on.

const SAMPLE_MS = 50;
const CALIBRATION_LIMIT = 1 << 16;

/// Which of three clocks this runtime has.
///
/// `real`     -- moves with the work, so a benchmark can time itself from in here.
/// `frozen`   -- never moves: the plain interpreter's virtual realm time, which advances only
///               for scheduled work (ADR 0044).
/// `stepping` -- moves by exactly one millisecond per *read* and not at all with the work. This
///               is the deterministic host clock the Node adapter installs: a fixed start and a
///               fixed step, so that a run is reproducible and two reads never collide.
///
/// The third one is why this function is not one line. A `stepping` clock looks exactly like a
/// working clock to any check that reads it either side of a workload -- it *did* advance -- and
/// the first version of this harness believed it. The calibration below then measured one
/// millisecond however much work it did, never reached its sample threshold, and doubled the
/// batch until a single sample was a million iterations of `buffer.mjs`. It ran for seventy-four
/// minutes before anyone looked.
///
/// So the question is asked in the one way that separates all three: read the clock several
/// times with no work at all in between. Only a clock that steps per read moves then.
function clockKind() {
  const reads = [];
  for (let i = 0; i < 5; i += 1) reads.push(Date.now());
  let stepping = true;
  for (let i = 1; i < reads.length; i += 1) {
    if (reads[i] - reads[i - 1] !== 1) {
      stepping = false;
      break;
    }
  }
  if (stepping) return "stepping";
  const before = Date.now();
  let sink = 0;
  for (let i = 0; i < 3000000; i += 1) sink += i;
  return Date.now() - before > 0 && sink > 0 ? "real" : "frozen";
}

function report(fields) {
  console.log(JSON.stringify(fields));
}

function median(sorted) {
  return sorted[sorted.length >> 1];
}

/// Everything a benchmark pays before its workload runs, and nothing beyond it.
///
/// Process start, module load, and classifying the clock -- which is not free, because deciding
/// that a clock does not move means doing work and watching it not move. `compare.sh` subtracts
/// this from an externally timed run.
///
/// Deliberately not `bench(name, () => 0)`: that pays a full measurement budget too, so the floor
/// came out at two seconds and every externally timed row had two seconds taken off it. Two of
/// them went negative and reported 0.00 ms, which is a benchmark claiming a workload took no time
/// at all -- the exact shape of wrong number this harness exists to stop.
export function floor() {
  const kind = clockKind();
  console.log(JSON.stringify({
    name: "floor", mode: kind === "real" ? "in-process" : "external",
    clock: kind, iterations: 1, checksum: "0",
  }));
}

/// Run `workload` until `budgetMs` has passed, reporting the first call apart from the rest.
///
/// Returns a promise when the workload does, so an async benchmark needs no special spelling at
/// the call site and no top-level await.
///
/// `options.checksum`, when given, turns the last result into the reported checksum. It runs
/// once, after the clock has stopped, so a workload can return the array it built and have it
/// summed outside the measurement rather than paying a sequential pass inside every call.
export function bench(name, workload, options = {}) {
  const budgetMs = options.budgetMs ?? 2000;
  const iterations = options.externalIterations ?? 1;
  const summarise = options.checksum ?? ((value) => value);

  if (clockKind() !== "real") {
    // Nothing here can be timed from the inside. Do a known amount of work and let the runner
    // time the process; reporting a zero would be worse than reporting no number at all.
    const done = (sink) => report({ name, mode: "external", iterations, checksum: String(summarise(sink)) });
    let sink;
    let chain = null;
    for (let i = 0; i < iterations; i += 1) {
      if (chain) {
        chain = chain.then(() => workload()).then((value) => { sink = value; });
      } else {
        sink = workload();
        if (sink && typeof sink.then === "function") {
          chain = sink.then((value) => { sink = value; });
        }
      }
    }
    return chain ? chain.then(() => done(sink)) : done(sink);
  }

  const first = Date.now();
  const cold = workload();
  if (cold && typeof cold.then === "function") {
    return cold.then((value) => timedAsync(name, workload, budgetMs, Date.now() - first, value, summarise));
  }
  return timedSync(name, workload, budgetMs, Date.now() - first, cold, summarise);
}

function timedSync(name, workload, budgetMs, cold, coldValue, summarise) {
  let sink = coldValue;
  let batch = 1;
  // Bounded twice over. `clockKind` has already ruled out the clock that cannot answer this, and
  // the cap is the belt to that pair of braces: an unbounded doubling here is what turned a
  // misread clock into a seventy-four minute hang rather than a wrong number.
  for (;;) {
    const started = Date.now();
    for (let i = 0; i < batch; i += 1) sink = workload();
    if (Date.now() - started >= SAMPLE_MS || batch >= CALIBRATION_LIMIT) break;
    batch *= 2;
  }
  const samples = [];
  const deadline = Date.now() + budgetMs;
  while (Date.now() < deadline) {
    const started = Date.now();
    for (let i = 0; i < batch; i += 1) sink = workload();
    samples.push((Date.now() - started) / batch);
  }
  samples.sort((a, b) => a - b);
  report({
    name, mode: "in-process", cold, warm_median: median(samples), warm_best: samples[0],
    batch, samples: samples.length, checksum: String(summarise(sink)),
  });
}

async function timedAsync(name, workload, budgetMs, cold, coldValue, summarise) {
  let sink = coldValue;
  let batch = 1;
  for (;;) {
    const started = Date.now();
    for (let i = 0; i < batch; i += 1) sink = await workload();
    if (Date.now() - started >= SAMPLE_MS || batch >= CALIBRATION_LIMIT) break;
    batch *= 2;
  }
  const samples = [];
  const deadline = Date.now() + budgetMs;
  while (Date.now() < deadline) {
    const started = Date.now();
    for (let i = 0; i < batch; i += 1) sink = await workload();
    samples.push((Date.now() - started) / batch);
  }
  samples.sort((a, b) => a - b);
  report({
    name, mode: "in-process", cold, warm_median: median(samples), warm_best: samples[0],
    batch, samples: samples.length, checksum: String(summarise(sink)),
  });
}
