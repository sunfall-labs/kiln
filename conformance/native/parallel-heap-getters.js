// kiln-native: profile=native-release parallelism=1,2,max
//
// Tier 2 (.scratch/parallel-builtins issue 07): a read that could run user code stays
// sequential. A getter anywhere on the path -- own, on a class prototype, installed with
// `Object.defineProperty` where the compiler cannot see it, on one element in the middle of the
// array -- a proxy, and an `arguments` object each make the read decline, and the call runs the
// sequential method from the first element. Every getter logs its calls, and the log must be the
// sequential method's: the same calls, in the same order, once each.
function check(label, p, m, pLog, mLog) {
  let ok = p.length === m.length && pLog.length === mLog.length;
  for (let i = 0; ok && i < p.length; i++) ok = i in p === i in m && Object.is(p[i], m[i]);
  for (let i = 0; ok && i < pLog.length; i++) ok = pLog[i] === mLog[i];
  console.log(label, p.length, String(p[0]), String(p[p.length - 1]), pLog.length, ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallel differs from sequential: " + label);
}

let log = [];
const n = 40000;
const plain = [];
for (let i = 0; i < n; i++) plain.push({ a: i, w: i % 9 });
// One element near the end has a getter the compiler cannot see, which the pool meets after
// running tens of thousands of elements: the call starts again, sequentially, from the first.
const late = plain.slice();
late[n - 7] = { w: 2 };
Object.defineProperty(late[n - 7], "a", { get: () => (log.push("late"), -1) });
class Box {
  constructor(i) {
    this.raw = i;
  }
  get v() {
    log.push(this.raw);
    return this.raw * 2;
  }
}
const boxes = [];
for (let i = 0; i < 3000; i++) boxes.push(new Box(i));
const hidden = [];
for (let i = 0; i < n; i++) {
  const o = { w: i };
  Object.defineProperty(o, "u", { get: () => (log.push(i), i + 1), enumerable: true });
  hidden.push(o);
}
const proxied = [];
for (let i = 0; i < 3000; i++) proxied.push(new Proxy({ u: i }, { get: (t, k) => (log.push(k), t[k]) }));
function args() {
  return arguments;
}
const argsList = [];
for (let i = 0; i < 3000; i++) argsList.push(args(i, i + 1));

// Each site is written out: the compiler lowers a call whose callback it can see.
function logs(f) {
  log = [];
  const answer = f();
  const seen = log;
  log = [];
  return [answer, seen];
}
let p, pLog, m, mLog;
[p, pLog] = logs(() => late.parallelMap(x => x.a + x.w));
[m, mLog] = logs(() => late.map(x => x.a + x.w));
check("getter on one late element", p, m, pLog, mLog);
[p, pLog] = logs(() => boxes.parallelMap(x => x.v));
[m, mLog] = logs(() => boxes.map(x => x.v));
check("getter on the class prototype", p, m, pLog, mLog);
[p, pLog] = logs(() => hidden.parallelMap(x => x.u * 2));
[m, mLog] = logs(() => hidden.map(x => x.u * 2));
check("defineProperty getter", p, m, pLog, mLog);
[p, pLog] = logs(() => proxied.parallelMap(x => x.u % 2));
[m, mLog] = logs(() => proxied.map(x => x.u % 2));
check("proxy elements", p, m, pLog, mLog);
[p, pLog] = logs(() => argsList.parallelMap(x => x[0] + x[1]));
[m, mLog] = logs(() => argsList.map(x => x[0] + x[1]));
check("arguments objects", p, m, pLog, mLog);
[p, pLog] = logs(() => plain.parallelMap(x => x.a + x.w));
[m, mLog] = logs(() => plain.map(x => x.a + x.w));
check("no getter reached", p, m, pLog, mLog);
