// Tick ordering of async generator yields and thenable adoption, verifying parity
// between interpreter and native engine against node's execution order.
const log = [];

async function a() {
  log.push("a0");
  await 1;
  log.push("a1");
  await Promise.resolve(2);
  log.push("a2");
  try {
    await Promise.reject(new Error("boom"));
  } catch (e) {
    log.push("a3:" + e.message);
  }
}

async function* gen() {
  log.push("g0");
  yield await 10;
  log.push("g1");
  yield 21;
  try {
    yield Promise.reject(new Error("gen-rejected-yield"));
  } catch (e) {
    log.push("g2:" + e.message);
  }
  yield 32;
}

async function c() {
  for await (const v of gen()) {
    log.push("c:" + v);
  }
}

async function main() {
  Promise.resolve()
    .then(() => log.push("t0"))
    .then(() => log.push("t1"))
    .then(() => log.push("t2"));

  await Promise.all([a(), c()]);

  const thenable = {
    then(res) {
      res("thenable");
    },
  };
  log.push(await thenable);

  const chainedThenable = await Promise.resolve().then(() => ({
    then(res) {
      res("chained-thenable");
    },
  }));
  log.push(chainedThenable);

  console.log(log.join(" "));
}

main();
