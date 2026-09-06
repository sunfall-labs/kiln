// The order of resumptions across `await` of plain values, settled promises, pending promises
// and rejections, against `then` chains and each other, as node orders them. An await resumes
// its frame directly now, without the two bound handlers and the derived promise it used to
// allocate, and the resumption must land on exactly the tick it always did.
const log = [];
async function a() {
  log.push("a0");
  await 1;
  log.push("a1");
  await Promise.resolve(2);
  log.push("a2");
  try {
    await Promise.reject(new Error("boom"));
    log.push("unreachable");
  } catch (e) {
    log.push("a3:" + e.message);
  }
  return "a-done";
}
async function b() {
  log.push("b0");
  const p = new Promise((resolve) => setTimeout(() => resolve("late"), 0));
  log.push("b1:" + (await p));
  return "b-done";
}
async function d() {
  await null;
  throw new Error("d-throws");
}
async function main() {
  Promise.resolve().then(() => log.push("t0")).then(() => log.push("t1")).then(() => log.push("t2"));
  const results = await Promise.all([a(), b(), d().catch((e) => "caught:" + e.message)]);
  log.push(results.join(","));
  // A pending promise awaited by two frames resumes both, in order.
  let release;
  const shared = new Promise((r) => { release = r; });
  async function w(n) { log.push("w" + n + ":" + (await shared)); }
  const ws = [w(1), w(2)];
  release("shared");
  await Promise.all(ws);
  let count = 0;
  for (let i = 0; i < 5000; i++) { await i; count += 1; }
  log.push("count:" + count);

  console.log(log.join(" "));
}
main();
